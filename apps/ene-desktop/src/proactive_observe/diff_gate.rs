//! Screen-diff gate that skips redundant vision inference when the screen
//! content has not changed significantly.
//!
//! Compares 64×64 luma fingerprints of the overview (changed-cell count) and
//! of the ROI crop (exact match). Average hashing was rejected after
//! simulation showed 8×8 block means are blind to the changes that matter:
//! a typed line and an identical 40–80px scroll both produced a Hamming
//! distance of 0.

use image::DynamicImage;

/// Fingerprint grid side length in cells (64×64 = 4096 cells).
const FINGERPRINT_GRID: u32 = 64;

/// Per-cell luminance delta (out of 255) above which a cell counts as
/// changed. ~2.4% separates sub-cell noise (caret blink, clock digits,
/// cursor sprite) from real content edits, which shift cell means by 10%+.
const CELL_DELTA: u8 = 6;

/// Maximum changed overview cells that still count as "unchanged". A
/// full-width text line changes 61 of 4096 cells in simulation; a 48px
/// cursor sprite at most 28, so the two are cleanly separated.
const OVERVIEW_CHANGED_CELL_LIMIT: usize = 48;

/// Cached screen state: fingerprints plus the summary they produced.
struct CachedScreen {
    app_label: String,
    overview: Vec<u8>,
    roi: Option<Vec<u8>>,
    summary: String,
}

/// Diff gate that caches the last screen fingerprint and summary.
///
/// # Usage
///
/// ```ignore
/// let mut gate = ScreenDiffGate::new();
///
/// // On every observation tick:
/// let overview = fingerprint(&overview_image);
/// let roi = roi_image.as_ref().map(fingerprint);
/// if let Some(cached) = gate.check(&overview, roi.as_deref(), &app_label) {
///     // Screen hasn't changed much — reuse previous summary.
///     return cached;
/// }
///
/// // … run expensive vision inference …
/// gate.cache(overview, roi, app_label.to_string(), new_summary);
/// ```
pub struct ScreenDiffGate {
    last: Option<CachedScreen>,
}

impl ScreenDiffGate {
    #[must_use]
    pub fn new() -> Self {
        Self { last: None }
    }

    /// Check whether the screen has changed significantly since the last
    /// [`Self::cache`] call. Returns `Some(cached_summary)` when the app label
    /// matches, fewer than [`OVERVIEW_CHANGED_CELL_LIMIT`] overview cells
    /// changed, and the ROI fingerprint (when present both times) matches
    /// exactly. A dimension or ROI-presence mismatch forces re-inference.
    ///
    #[must_use]
    pub fn check(&self, overview: &[u8], roi: Option<&[u8]>, app_label: &str) -> Option<&str> {
        let last = self.last.as_ref()?;
        if last.app_label != app_label || last.roi.as_deref() != roi {
            return None;
        }
        if changed_cells(&last.overview, overview) >= OVERVIEW_CHANGED_CELL_LIMIT {
            return None;
        }
        Some(&last.summary)
    }

    pub fn cache(
        &mut self,
        overview: Vec<u8>,
        roi: Option<Vec<u8>>,
        app_label: String,
        summary: String,
    ) {
        self.last = Some(CachedScreen {
            app_label,
            overview,
            roi,
            summary,
        });
    }
}

impl Default for ScreenDiffGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Downscale an image to the 64×64 grayscale fingerprint the gate compares.
pub fn fingerprint(img: &DynamicImage) -> Vec<u8> {
    img.resize_exact(
        FINGERPRINT_GRID,
        FINGERPRINT_GRID,
        image::imageops::FilterType::Lanczos3,
    )
    .to_luma8()
    .into_raw()
}

/// Count fingerprint cells whose luminance moved by at least [`CELL_DELTA`].
/// Different-length fingerprints (surface resized) count as fully changed.
fn changed_cells(previous: &[u8], current: &[u8]) -> usize {
    if previous.len() != current.len() {
        return usize::MAX;
    }
    previous
        .iter()
        .zip(current)
        .filter(|(a, b)| a.abs_diff(**b) >= CELL_DELTA)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_images_produce_same_hash() {
        let img = DynamicImage::new_rgba8(100, 100);
        let h1 = fingerprint(&img);
        let h2 = fingerprint(&img);
        assert_eq!(h1, h2);
    }

    #[test]
    fn gate_returns_none_when_empty() {
        let gate = ScreenDiffGate::new();
        let fp = fingerprint(&DynamicImage::new_rgba8(64, 64));
        assert!(gate.check(&fp, None, "app").is_none());
    }

    #[test]
    fn gate_returns_cached_when_identical() {
        let mut gate = ScreenDiffGate::new();
        let img = code_like_image(512, 512);
        let fp = fingerprint(&img);
        gate.cache(fp.clone(), None, "code".into(), "hello".into());
        assert_eq!(gate.check(&fp, None, "code"), Some("hello"));
    }

    #[test]
    fn gate_misses_on_app_switch() {
        let mut gate = ScreenDiffGate::new();
        let fp = fingerprint(&code_like_image(512, 512));
        gate.cache(fp.clone(), None, "code".into(), "hello".into());
        assert!(gate.check(&fp, None, "firefox").is_none());
    }

    #[test]
    fn gate_misses_on_typed_line() {
        let mut gate = ScreenDiffGate::new();
        let base = code_like_image(512, 512);
        let fp = fingerprint(&base);
        gate.cache(fp, None, "code".into(), "hello".into());

        // One extra full-width bright line: more than the overview limit.
        let mut edited = base;
        let pixels = edited.as_mut_rgba8().unwrap();
        for x in 0..pixels.width() {
            for y in (pixels.height() / 2)..(pixels.height() / 2 + 2) {
                pixels.put_pixel(x, y, image::Rgba([220, 220, 220, 255]));
            }
        }
        let edited_fp = fingerprint(&DynamicImage::ImageRgba8(pixels.clone()));
        assert!(gate.check(&edited_fp, None, "code").is_none());
    }

    #[test]
    fn gate_hits_on_cursor_sized_change() {
        let mut gate = ScreenDiffGate::new();
        let base = code_like_image(512, 512);
        let fp = fingerprint(&base);
        gate.cache(fp, None, "code".into(), "hello".into());

        // A 24x24 cursor sprite moves: only a handful of cells change.
        let mut edited = base;
        let pixels = edited.as_mut_rgba8().unwrap();
        for x in 100..124 {
            for y in 100..124 {
                pixels.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        let edited_fp = fingerprint(&DynamicImage::ImageRgba8(pixels.clone()));
        assert_eq!(gate.check(&edited_fp, None, "code"), Some("hello"));
    }

    #[test]
    fn gate_misses_when_roi_changes() {
        let mut gate = ScreenDiffGate::new();
        let img = code_like_image(512, 512);
        let ov = fingerprint(&img);
        let roi = fingerprint(&img);
        gate.cache(ov, Some(roi), "code".into(), "hello".into());

        let mut edited = img.clone();
        let pixels = edited.as_mut_rgba8().unwrap();
        for x in 200..220 {
            for y in 200..220 {
                pixels.put_pixel(x, y, image::Rgba([240, 240, 240, 255]));
            }
        }
        let edited_roi = fingerprint(&DynamicImage::ImageRgba8(pixels.clone()));
        assert!(
            gate.check(&fingerprint(&img), Some(&edited_roi), "code")
                .is_none()
        );
    }

    #[test]
    fn gate_misses_when_roi_presence_changes() {
        let mut gate = ScreenDiffGate::new();
        let img = code_like_image(512, 512);
        let ov = fingerprint(&img);
        gate.cache(
            ov.clone(),
            Some(fingerprint(&img)),
            "code".into(),
            "hello".into(),
        );
        assert!(gate.check(&ov, None, "code").is_none());
    }

    #[test]
    fn gate_misses_on_dimension_mismatch() {
        let mut gate = ScreenDiffGate::new();
        let fp = fingerprint(&code_like_image(512, 512));
        gate.cache(fp, None, "code".into(), "hello".into());
        let resized = fingerprint(&code_like_image(400, 400));
        assert!(gate.check(&resized, None, "code").is_none());
    }

    /// Dark code-editor-like image with text rows, used to exercise the gate.
    fn code_like_image(width: u32, height: u32) -> DynamicImage {
        let mut img = DynamicImage::new_rgba8(width, height);
        let pixels = img.as_mut_rgba8().unwrap();
        for y in (8..height).step_by(20) {
            for x in (8..width.saturating_sub(60)).step_by(11) {
                for k in 0..7 {
                    if x + k < width {
                        pixels.put_pixel(x + k, y, image::Rgba([180, 180, 180, 255]));
                    }
                }
            }
        }
        img
    }
}
