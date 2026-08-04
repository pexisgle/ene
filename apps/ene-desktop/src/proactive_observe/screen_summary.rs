//! Screen summary provider: capture → composite → local Gemma vision → drop image.
//!
//! Raw pixels stay in the desktop/runtime process. Enabling
//! `mind.proactive.sources.screen_summary` opts in to a short local
//! multimodal completion against the proactive GGUF + `mmproj`.
//! Vision failures mark the source unavailable — no fabricated summaries.
//!
//! The pipeline wires in the lightweight observers:
//! - [`super::capture::compose`] composites a 100%-scale cursor ROI next to
//!   the 50% overview so fine text near the pointer stays legible.
//! - [`super::diff_gate`] skips redundant vision inference when the screen
//!   fingerprints have not changed significantly, returning the cached
//!   summary instead.
//! - [`super::ocr`] flags code / terminal windows and exposes the OCR
//!   text-hint hook that a future local OCR backend can fill.

use std::time::{Duration, Instant};

use image::DynamicImage;
use parking_lot::Mutex;

use super::capture::{CapturedScreen, capture_for_summary, compose};
use super::diff_gate::{ScreenDiffGate, fingerprint};
use super::{redact_paths, truncate};
use ene_runtime::EneHandle;

/// Brief backoff after capture/vision failure (avoids hammering portals).
const FAIL_BACKOFF: Duration = Duration::from_secs(5);

/// Host screen summarizer (local Gemma + mmproj via runtime actor).
pub struct ScreenSummaryProvider {
    handle: EneHandle,
    last_failure_at: Mutex<Option<Instant>>,
    /// Fingerprint gate that caches the last summary so unchanged screens do
    /// not trigger a fresh vision inference.
    diff_gate: Mutex<ScreenDiffGate>,
}

impl ScreenSummaryProvider {
    /// Build with a handle into the runtime actor (reuses the decision model).
    #[must_use]
    pub fn new(handle: EneHandle) -> Self {
        Self {
            handle,
            last_failure_at: Mutex::new(None),
            diff_gate: Mutex::new(ScreenDiffGate::new()),
        }
    }

    /// Capture + summarize. Always takes a fresh frame. Returns `None` when unavailable.
    ///
    /// `cursor` is the global screen-space cursor position when known; it
    /// drives ROI cropping so the vision model focuses on the user's
    /// point of attention. `None` summarizes the full captured frame.
    pub async fn summarize(&self, max_chars: usize, cursor: Option<(i32, i32)>) -> Option<String> {
        let max_chars = max_chars.max(32);

        if let Some(failed_at) = *self.last_failure_at.lock()
            && failed_at.elapsed() < FAIL_BACKOFF
        {
            return None;
        }

        let captured = match capture_for_summary(cursor).await {
            Ok(c) => c,
            Err(e) => {
                tracing::info!(
                    component = "ProactiveObserve",
                    error = %e,
                    "Screen capture skipped"
                );
                *self.last_failure_at.lock() = Some(Instant::now());
                return None;
            }
        };
        let (composite, placed_overview) = compose(&captured.image, captured.roi.as_ref());
        let overview_fp = fingerprint(&placed_overview);
        let roi_fp = captured.roi.as_ref().map(fingerprint);
        if let Some(cached) =
            self.diff_gate
                .lock()
                .check(&overview_fp, roi_fp.as_deref(), &captured.app_label)
        {
            tracing::info!(
                component = "ProactiveObserve",
                event = "screen_diff_gate",
                cached = true,
                app = %captured.app_label,
                "Screen unchanged; reusing cached summary"
            );
            return Some(truncate(&redact_paths(cached), max_chars));
        }

        let text = match self.summarize_captured(&captured, &composite).await {
            Ok(t) => t,
            Err(e) => {
                if matches!(e, ene_runtime::PublicApiError::ActorDead)
                    || e.to_string().contains("runtime busy")
                    || e.to_string().contains("cancelled")
                {
                    tracing::debug!(
                        component = "ProactiveObserve",
                        detail = %e,
                        "Local vision summary skipped during active turn"
                    );
                } else {
                    tracing::warn!(
                        component = "ProactiveObserve",
                        error = %e,
                        width = captured.image.width(),
                        height = captured.image.height(),
                        "Local vision summary failed; screen source unavailable"
                    );
                }
                *self.last_failure_at.lock() = Some(Instant::now());
                return None;
            }
        };

        let text = truncate(&redact_paths(&text), max_chars);
        if text.trim().is_empty() {
            *self.last_failure_at.lock() = Some(Instant::now());
            return None;
        }

        tracing::info!(
            component = "ProactiveObserve",
            chars = text.chars().count(),
            app = %captured.app_label,
            "Screen summary refreshed"
        );

        *self.last_failure_at.lock() = None;
        self.diff_gate.lock().cache(
            overview_fp,
            roi_fp,
            captured.app_label.clone(),
            text.clone(),
        );
        Some(text)
    }

    async fn summarize_captured(
        &self,
        captured: &CapturedScreen,
        composite: &DynamicImage,
    ) -> Result<String, ene_runtime::PublicApiError> {
        // OCR text-hint hook: a future local OCR backend fills this in; the
        // hints ride along to the vision prompt once available.
        let ocr_hint = captured
            .roi
            .as_ref()
            .and_then(super::ocr::extract_text_hints);
        if let Some(hints) = ocr_hint.as_deref() {
            tracing::debug!(
                component = "ProactiveObserve",
                chars = hints.chars().count(),
                "Extracted text hints from focus region"
            );
        }
        let rgb = composite.to_rgb8();
        let (width, height) = rgb.dimensions();
        self.handle
            .vision()
            .summarize_screen_image(
                width,
                height,
                rgb.into_raw(),
                captured.app_label.clone(),
                ene_runtime::vision::ScreenSummaryHints {
                    roi_composited: captured.roi.is_some(),
                    code_window: captured.is_code_like,
                    ocr_text: ocr_hint,
                },
            )
            .await
    }
}
