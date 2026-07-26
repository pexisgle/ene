//! Screen summary provider: capture → local Gemma vision → drop image (#168).
//!
//! Raw pixels stay in the desktop/runtime process. Enabling
//! `mind.proactive.sources.screen_summary` opts in to a short local
//! multimodal completion against the proactive GGUF + `mmproj`.
//! Vision failures mark the source unavailable — no fabricated summaries.
//!
//! The pipeline wires in the lightweight observers from #215:
//! - [`super::diff_gate`] skips redundant vision inference when the
//!   screen hash has not changed significantly, returning the cached
//!   summary instead.
//! - [`super::ocr`] flags code / terminal windows so the signal can be
//!   logged alongside the summary.
//! - [`super::roi`] crops a region around the cursor (when known) so the
//!   vision model receives higher-detail pixels near the user's focus.

use std::time::{Duration, Instant};

use image::DynamicImage;
use parking_lot::Mutex;

use super::capture::{CapturedScreen, capture_for_summary};
use super::diff_gate::{ScreenDiffGate, average_hash};
use super::roi::crop_roi;
use super::{redact_paths, truncate};
use ene_runtime::EneHandle;

/// Brief backoff after capture/vision failure (avoids hammering portals).
const FAIL_BACKOFF: Duration = Duration::from_secs(5);

/// Host screen summarizer (local Gemma + mmproj via runtime actor).
pub struct ScreenSummaryProvider {
    handle: EneHandle,
    last_failure_at: Mutex<Option<Instant>>,
    /// Perceptual-hash gate that caches the last summary so unchanged
    /// screens do not trigger a fresh vision inference (#215).
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

        // Diff gate: reuse the cached summary when the screen has not
        // changed significantly since the last inference (#215).
        let hash = average_hash(&captured.image);
        if let Some(cached) = self.diff_gate.lock().check(hash) {
            tracing::debug!(
                component = "ProactiveObserve",
                app = %captured.app_label,
                "Screen unchanged; reusing cached summary"
            );
            return Some(cached.to_string());
        }

        if super::ocr::is_code_window(&captured.app_label, &captured.app_label) {
            tracing::debug!(
                component = "ProactiveObserve",
                app = %captured.app_label,
                "Active window looks like code/terminal"
            );
        }

        let text = match self.summarize_captured(&captured, cursor).await {
            Ok(t) => t,
            Err(e) => {
                if e.contains("runtime busy")
                    || e.contains("cancelled")
                    || e.contains("actor dropped reply")
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
        self.diff_gate.lock().cache(hash, text.clone());
        Some(text)
    }

    async fn summarize_captured(
        &self,
        captured: &CapturedScreen,
        cursor: Option<(i32, i32)>,
    ) -> Result<String, String> {
        // When the cursor position is known, crop a region of interest
        // around it so the vision model receives higher-detail pixels
        // near the user's focus (#215). Falls back to the full frame
        // when the crop is unavailable.
        let focus: DynamicImage = match cursor.and_then(|(x, y)| crop_roi(&captured.image, x, y)) {
            Some(roi) => roi,
            None => captured.image.clone(),
        };
        // OCR pre-filter hook (#215): surface any extracted text hints
        // from the focus region. The current implementation is a
        // placeholder that returns `None`; wiring the call now keeps the
        // pipeline ready for a real OCR backend.
        if let Some(hints) = super::ocr::extract_text_hints(&focus) {
            tracing::debug!(
                component = "ProactiveObserve",
                chars = hints.chars().count(),
                "Extracted text hints from focus region"
            );
        }
        let rgb = focus.to_rgb8();
        let (width, height) = rgb.dimensions();
        self.handle
            .summarize_screen_image(width, height, rgb.into_raw(), captured.app_label.clone())
            .await
    }
}
