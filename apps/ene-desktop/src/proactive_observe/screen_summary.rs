//! Screen summary provider: capture → local Gemma vision → drop image (#168).
//!
//! Raw pixels stay in the desktop/runtime process. Enabling
//! `mind.proactive.sources.screen_summary` opts in to a short local
//! multimodal completion against the proactive GGUF + `mmproj`.
//! Vision failures mark the source unavailable — no fabricated summaries.
//! Every summarize call captures a fresh frame (no cross-tick cache).

use std::time::{Duration, Instant};

use ene_runtime::EneHandle;
use parking_lot::Mutex;

use super::capture::{CapturedScreen, capture_for_summary};
use super::{redact_paths, truncate};

/// Brief backoff after capture/vision failure (avoids hammering portals).
const FAIL_BACKOFF: Duration = Duration::from_secs(5);

/// Host screen summarizer (local Gemma + mmproj via runtime actor).
pub struct ScreenSummaryProvider {
    handle: EneHandle,
    last_failure_at: Mutex<Option<Instant>>,
}

impl ScreenSummaryProvider {
    /// Build with a handle into the runtime actor (reuses the decision model).
    #[must_use]
    pub fn new(handle: EneHandle) -> Self {
        Self {
            handle,
            last_failure_at: Mutex::new(None),
        }
    }

    /// Capture + summarize. Always takes a fresh frame. Returns `None` when unavailable.
    pub async fn summarize(&self, max_chars: usize) -> Option<String> {
        let max_chars = max_chars.max(32);

        if let Some(failed_at) = *self.last_failure_at.lock()
            && failed_at.elapsed() < FAIL_BACKOFF
        {
            return None;
        }

        let captured = match capture_for_summary().await {
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

        let text = match self.summarize_captured(&captured).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(
                    component = "ProactiveObserve",
                    error = %e,
                    width = captured.image.width(),
                    height = captured.image.height(),
                    "Local vision summary failed; screen source unavailable"
                );
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
        Some(text)
    }

    async fn summarize_captured(&self, captured: &CapturedScreen) -> Result<String, String> {
        let rgb = captured.image.to_rgb8();
        let (width, height) = rgb.dimensions();
        self.handle
            .summarize_screen_image(
                width,
                height,
                rgb.into_raw(),
                captured.app_label.clone(),
            )
            .await
    }
}
