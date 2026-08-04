//! Screen-image vision summarization, decoupled from the turn-execution
//! actor mailbox.
//!
//! Before this split, `EneCommand::SummarizeScreenImage` carried a raw RGB8
//! buffer (`width * height * 3` bytes, up to several megabytes for a
//! 1920x1080 capture) through the actor's command enum, and the actual
//! (potentially multi-second) vision-model inference ran inside the actor's
//! `vision_tasks` `JoinSet` — a `JoinSet` whose own doc comment admitted it
//! existed only because the work "must not block the command loop".
//!
//! [`VisionHandle`] removes both problems: the raw buffer never enters
//! [`crate::handle::EneCommand`], and the expensive `summarize_rgb` call
//! happens directly here, awaited by the caller, never inside the actor's
//! command loop or any actor-owned `JoinSet`.
//!
//! What *does* still cross the mailbox (deliberately, see the PR
//! description for the tradeoff) is a small, payload-free
//! `PrepareVisionSummary` request/reply pair: the vision path shares the
//! same "runtime busy" gate (`active_turn` / in-flight proactive decision)
//! and the same lazily-initialized local GGUF vision model as the proactive
//! scheduler, both of which are actor-owned state. `PrepareVisionSummary`
//! performs that busy-check and lazy init, then hands back a cheap
//! `Arc`-cloned model handle plus the rendered system/user prompts. A
//! second fire-and-forget message, `StashProactiveScreenImage`, hands the
//! actor the *encoded* JPEG data URI (not the raw buffer) so the next
//! proactive generation turn can still attach it, preserving prior
//! behavior.

use crate::handle::EneCommand;
use crate::public_api::PublicApiError;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Maximum accepted pixel count for a screen capture (1920x1080).
const MAX_PIXELS: u64 = 1920 * 1080;

/// Result of a successful [`EneCommand::PrepareVisionSummary`] round trip:
/// everything [`VisionHandle::summarize_screen_image`] needs to run the
/// actual vision inference outside the actor.
pub struct VisionPrepared {
    /// Cloned handle to the local vision-capable model.
    pub local: Arc<ene_ai_local::LocalLlamaCppProvider>,
    /// Rendered system prompt for the screen-summary task.
    pub system: String,
    /// Rendered user prompt (includes the privacy-safe app label).
    pub user: String,
    /// Cancellation token for this specific vision request. The actor mints
    /// a fresh token per [`EneCommand::PrepareVisionSummary`] reply and
    /// cancels it when a new user turn starts, so a long-running vision
    /// inference does not keep the local model busy behind a request the
    /// user has already moved past. Threaded into `ene_infer::JobContext`
    /// (via `summarize_rgb_with_cancel`'s `EngineHandle::submit` call) and
    /// checked cooperatively by the worker, not by this actor's mailbox.
    pub cancel: CancellationToken,
}

/// Handle for screen-image vision summarization.
///
/// Obtained via [`crate::EneHandle::vision`]. Cheap to clone (wraps a
/// single `Arc`d command sender).
#[derive(Clone)]
pub struct VisionHandle {
    cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
}

/// Non-image context for the screen-summary vision call.
///
/// All fields are small text / flags; raw pixels never cross the mailbox.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenSummaryHints {
    /// The frame composites a 100%-scale cursor ROI next to the 50% overview.
    pub roi_composited: bool,
    /// The active window looks like a code editor / terminal (title heuristics).
    pub code_window: bool,
    /// Raw OCR text extracted from the focus region; `None` when no OCR
    /// backend produced a hint.
    pub ocr_text: Option<String>,
}

impl VisionHandle {
    pub(crate) fn new(cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>) -> Self {
        Self { cmd_tx }
    }

    /// Summarize a screen RGB8 capture via the local vision (mmproj) model.
    ///
    /// Part of the API v1 contract: errors are the stable
    /// [`PublicApiError`] categories, not a bare `String`.
    ///
    /// The `rgb` buffer and the actual model call never touch the actor's
    /// command mailbox — see the module docs for what small, payload-free
    /// messages still do.
    pub async fn summarize_screen_image(
        &self,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
        app_label: String,
        hints: ScreenSummaryHints,
    ) -> Result<String, PublicApiError> {
        validate_rgb(width, height, &rgb)?;

        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::PrepareVisionSummary {
                app_label,
                hints,
                reply,
            })
            .map_err(|_| PublicApiError::ActorDead)?;
        let prepared = rx.await.map_err(|_| PublicApiError::ActorDead)??;

        // Best-effort stash of the *encoded* frame for the next proactive
        // generation turn (never the raw buffer, and never blocks this
        // call or the actor on failure).
        let stash = crate::proactive::rgb_to_jpeg_data_uri(width, height, &rgb).ok();
        if stash.is_none() {
            tracing::warn!(
                component = "Proactive",
                "Failed to stash screen frame for generation; continuing text-only"
            );
        }
        drop(
            self.cmd_tx
                .send(EneCommand::StashProactiveScreenImage { data_uri: stash }),
        );

        // The actual (potentially multi-second) inference call happens
        // here, entirely outside the actor's command loop. `prepared.cancel`
        // lets a new user turn abort it early (see `VisionPrepared::cancel`).
        prepared
            .local
            .summarize_rgb_with_cancel(
                width,
                height,
                rgb,
                &prepared.system,
                &prepared.user,
                prepared.cancel,
            )
            .await
            .map_err(|e| PublicApiError::Internal {
                message: e.to_string(),
            })
    }
}

/// Validates a screen-capture RGB8 buffer's dimensions and length before it
/// is sent anywhere. Pure and actor-independent.
fn validate_rgb(width: u32, height: u32, rgb: &[u8]) -> Result<(), PublicApiError> {
    let width_u = u64::from(width);
    let height_u = u64::from(height);
    if width == 0 || height == 0 {
        return Err(PublicApiError::Invalid {
            message: "invalid screen image dimensions".to_string(),
        });
    }
    if width_u.saturating_mul(height_u) > MAX_PIXELS {
        return Err(PublicApiError::Invalid {
            message: format!("screen image too large ({width}x{height}; max {MAX_PIXELS} pixels)"),
        });
    }
    let expected = width_u.saturating_mul(height_u).saturating_mul(3);
    let Ok(expected_len) = usize::try_from(expected) else {
        return Err(PublicApiError::Invalid {
            message: "screen image byte length overflows usize".to_string(),
        });
    };
    if rgb.len() != expected_len {
        return Err(PublicApiError::Invalid {
            message: format!(
                "rgb buffer length mismatch (got {}, expected {expected_len})",
                rgb.len()
            ),
        });
    }
    Ok(())
}
