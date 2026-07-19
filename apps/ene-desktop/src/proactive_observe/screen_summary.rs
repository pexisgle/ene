//! Screen summary provider: capture → local Gemma vision → drop image (#168).
//!
//! Raw pixels stay in the desktop/runtime process. Enabling
//! `mind.proactive.sources.screen_summary` opts in to a short local
//! multimodal completion against the proactive GGUF + `mmproj`.

use std::time::{Duration, Instant};

use ene_runtime::EneHandle;
use parking_lot::Mutex;

use super::capture::{CapturedScreen, capture_for_summary};
use super::{redact_paths, truncate};

/// Minimum time between vision refreshes when the focused app is unchanged.
const MIN_REFRESH: Duration = Duration::from_mins(1);

#[derive(Debug, Clone)]
struct CachedSummary {
    text: String,
    app_label: String,
    captured_at: Instant,
}

/// Host screen summarizer (local Gemma + mmproj via runtime actor).
pub struct ScreenSummaryProvider {
    handle: EneHandle,
    cache: Mutex<Option<CachedSummary>>,
}

impl ScreenSummaryProvider {
    /// Build with a handle into the runtime actor (reuses the decision model).
    #[must_use]
    pub fn new(handle: EneHandle) -> Self {
        Self {
            handle,
            cache: Mutex::new(None),
        }
    }

    /// Capture + summarize. Returns `None` when unavailable.
    pub async fn summarize(&self, max_chars: usize) -> Option<String> {
        let max_chars = max_chars.max(32);
        let app_hint = active_app_hint();
        if let Some(cached) = self.cache.lock().as_ref()
            && cached.app_label == app_hint
            && cached.captured_at.elapsed() < MIN_REFRESH
        {
            return Some(truncate(&cached.text, max_chars));
        }

        let captured = match capture_for_summary().await {
            Ok(c) => c,
            Err(e) => {
                tracing::info!(
                    component = "ProactiveObserve",
                    error = %e,
                    "Screen capture skipped"
                );
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
                    "Local vision summary failed; using metadata fallback"
                );
                metadata_fallback(&captured)
            }
        };

        let text = truncate(&redact_paths(&text), max_chars);
        if text.trim().is_empty() {
            return None;
        }

        tracing::info!(
            component = "ProactiveObserve",
            chars = text.chars().count(),
            app = %captured.app_label,
            "Screen summary refreshed"
        );

        *self.cache.lock() = Some(CachedSummary {
            text: text.clone(),
            app_label: if captured.app_label.is_empty() {
                app_hint
            } else {
                captured.app_label.clone()
            },
            captured_at: Instant::now(),
        });
        Some(text)
    }

    async fn summarize_captured(&self, captured: &CapturedScreen) -> Result<String, String> {
        let rgb = captured.image.to_rgb8();
        let (width, height) = rgb.dimensions();
        self.handle
            .summarize_screen_image(width, height, rgb.into_raw())
            .await
    }
}

fn metadata_fallback(captured: &CapturedScreen) -> String {
    let w = captured.image.width();
    let h = captured.image.height();
    if captured.app_label.is_empty() {
        format!("Screen capture available ({w}x{h}).")
    } else {
        format!(
            "Active application: {}. Screen capture available ({w}x{h}).",
            captured.app_label
        )
    }
}

fn active_app_hint() -> String {
    match active_win_pos_rs::get_active_window() {
        Ok(win) => redact_paths(win.app_name.trim()),
        Err(()) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::DynamicImage;

    #[test]
    fn metadata_fallback_includes_app() {
        let captured = CapturedScreen {
            image: DynamicImage::new_rgba8(8, 4),
            app_label: "firefox".into(),
        };
        let text = metadata_fallback(&captured);
        assert!(text.contains("firefox"));
        assert!(text.contains("8x4"));
    }
}
