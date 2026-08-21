//! Screen-change gate: luma fingerprint, ROI crop, caret/clock suppression.

use ene_companion::{ObservationTitleMode, WorldStateSettings, redact_window_title};
use image::{ExtendedColorType, ImageEncoder, ImageReader, Limits, RgbaImage, imageops};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::io::Cursor;
use thiserror::Error;

const GRID_W: u32 = 48;
const GRID_H: u32 = 27;
const LUMA_DELTA: u8 = 12;
const CARET_MAX_CELLS: usize = 3;
const CLOCK_MAX_CELLS: usize = 8;
const OVERVIEW_MAX_EDGE: u32 = 512;
const ROI_EDGE: u32 = 256;
const RECENT_DIGESTS: usize = 6;
const MAX_DECODE_EDGE: u32 = 4_096;
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Failures when decoding a screenshot for the observation gate.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ObserveError {
    #[error("screenshot is not a decodable PNG")]
    InvalidPng,
    #[error("screenshot exceeds the decode size cap")]
    TooLarge,
    #[error("failed to encode observation PNG")]
    Encode,
}

/// Why a frame did not go to the vision model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObserveSkip {
    Unchanged,
    CaretBlink,
    Clock,
    PendingSmall,
}

/// Gate result after comparing the current frame to the in-memory luma cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObserveAction {
    /// Reuse the previous summary. Raw pixels stay off the wire.
    Skip {
        reason: ObserveSkip,
        summary: Option<String>,
        digest: String,
    },
    /// Call vision with the size-capped overview (and optional ROI crop).
    Changed {
        digest: String,
        overview_png: Vec<u8>,
        roi_png: Option<Vec<u8>>,
        roi_composited: bool,
    },
}

/// In-process luma cache. Stores digest + last summary, never the PNG.
#[derive(Debug, Default, Clone)]
pub struct ObservationPipeline {
    last_luma: Option<Vec<u8>>,
    recent: VecDeque<String>,
    last_summary: Option<String>,
    pending_digest: Option<String>,
}

impl ObservationPipeline {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Compare `png` to the previous luma grid and decide whether vision is needed.
    ///
    /// # Errors
    ///
    /// Returns [`ObserveError`] when the PNG cannot be decoded or re-encoded.
    pub fn evaluate(&mut self, png: &[u8]) -> Result<ObserveAction, ObserveError> {
        let frame = decode_frame(png)?;
        let digest = frame.digest.clone();
        let action = self.decide(&frame);
        if should_commit_luma(&action) {
            self.last_luma = Some(frame.luma);
        }
        self.recent.push_back(digest);
        while self.recent.len() > RECENT_DIGESTS {
            self.recent.pop_front();
        }
        Ok(action)
    }

    /// Remember the vision summary so an unchanged later frame can reuse it.
    pub fn commit_summary(&mut self, summary: String) {
        self.last_summary = Some(summary);
        self.pending_digest = None;
    }

    #[must_use]
    pub fn last_summary(&self) -> Option<&str> {
        self.last_summary.as_deref()
    }

    fn decide(&mut self, frame: &DecodedFrame) -> ObserveAction {
        let Some(prev) = self.last_luma.as_deref() else {
            self.pending_digest = None;
            return changed(frame, None);
        };
        let changed_cells = count_changed(prev, &frame.luma);
        if changed_cells == 0 {
            self.pending_digest = None;
            return self.skip(ObserveSkip::Unchanged, frame.digest.clone());
        }
        if self.is_oscillation(&frame.digest) {
            self.pending_digest = None;
            return self.skip(ObserveSkip::CaretBlink, frame.digest.clone());
        }
        if changed_cells <= CARET_MAX_CELLS {
            return self.small_change(frame, changed_cells);
        }
        if changed_cells <= CLOCK_MAX_CELLS && self.pending_digest.is_some() {
            self.pending_digest = Some(frame.digest.clone());
            return self.skip(ObserveSkip::Clock, frame.digest.clone());
        }
        self.pending_digest = None;
        let roi = roi_from_diff(prev, &frame.luma, frame.width, frame.height, &frame.rgba);
        changed(frame, roi)
    }

    fn small_change(&mut self, frame: &DecodedFrame, changed_cells: usize) -> ObserveAction {
        match self.pending_digest.as_deref() {
            None => {
                self.pending_digest = Some(frame.digest.clone());
                self.skip(ObserveSkip::PendingSmall, frame.digest.clone())
            }
            Some(pending) if pending == frame.digest => {
                self.pending_digest = None;
                let roi = self.last_luma.as_deref().and_then(|prev| {
                    roi_from_diff(prev, &frame.luma, frame.width, frame.height, &frame.rgba)
                });
                changed(frame, roi)
            }
            Some(_) if changed_cells <= CARET_MAX_CELLS => {
                self.pending_digest = Some(frame.digest.clone());
                self.skip(ObserveSkip::Clock, frame.digest.clone())
            }
            Some(_) => {
                self.pending_digest = Some(frame.digest.clone());
                self.skip(ObserveSkip::Clock, frame.digest.clone())
            }
        }
    }

    fn is_oscillation(&self, digest: &str) -> bool {
        self.recent
            .iter()
            .rev()
            .skip(1)
            .take(3)
            .any(|previous| previous == digest)
    }

    fn skip(&self, reason: ObserveSkip, digest: String) -> ObserveAction {
        ObserveAction::Skip {
            reason,
            summary: self.last_summary.clone(),
            digest,
        }
    }
}

fn should_commit_luma(action: &ObserveAction) -> bool {
    match action {
        ObserveAction::Changed { .. } => true,
        ObserveAction::Skip { reason, .. } => {
            matches!(reason, ObserveSkip::Unchanged | ObserveSkip::CaretBlink)
        }
    }
}

fn changed(frame: &DecodedFrame, roi_png: Option<Vec<u8>>) -> ObserveAction {
    ObserveAction::Changed {
        digest: frame.digest.clone(),
        overview_png: frame.overview_png.clone(),
        roi_composited: roi_png.is_some(),
        roi_png,
    }
}

struct DecodedFrame {
    digest: String,
    luma: Vec<u8>,
    width: u32,
    height: u32,
    overview_png: Vec<u8>,
    rgba: RgbaImage,
}

fn decode_frame(png: &[u8]) -> Result<DecodedFrame, ObserveError> {
    if png.len() < PNG_MAGIC.len() || !png.starts_with(PNG_MAGIC) {
        return Err(ObserveError::InvalidPng);
    }
    let mut reader = ImageReader::new(Cursor::new(png))
        .with_guessed_format()
        .map_err(|_| ObserveError::InvalidPng)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODE_EDGE);
    limits.max_image_height = Some(MAX_DECODE_EDGE);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|_| ObserveError::InvalidPng)?;
    let rgba = image.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    if width == 0 || height == 0 {
        return Err(ObserveError::InvalidPng);
    }
    if width > MAX_DECODE_EDGE || height > MAX_DECODE_EDGE {
        return Err(ObserveError::TooLarge);
    }
    let luma = luma_grid(&rgba);
    let digest = digest_luma(&luma);
    let overview = fit_rgba(&rgba, OVERVIEW_MAX_EDGE);
    let overview_png = encode_png(&overview)?;
    Ok(DecodedFrame {
        digest,
        luma,
        width,
        height,
        overview_png,
        rgba,
    })
}

fn luma_grid(img: &RgbaImage) -> Vec<u8> {
    let mut grid = vec![0_u8; usize::try_from(GRID_W.saturating_mul(GRID_H)).unwrap_or(0)];
    let width = img.width();
    let height = img.height();
    for gy in 0..GRID_H {
        for gx in 0..GRID_W {
            let x0 = gx.saturating_mul(width) / GRID_W;
            let x1 =
                (gx.saturating_add(1).saturating_mul(width) / GRID_W).max(x0.saturating_add(1));
            let y0 = gy.saturating_mul(height) / GRID_H;
            let y1 =
                (gy.saturating_add(1).saturating_mul(height) / GRID_H).max(y0.saturating_add(1));
            let mut sum = 0_u64;
            let mut count = 0_u64;
            for y in y0..y1.min(height) {
                for x in x0..x1.min(width) {
                    sum = sum.saturating_add(pixel_luma(img.get_pixel(x, y).0));
                    count = count.saturating_add(1);
                }
            }
            let avg = sum.checked_div(count).unwrap_or(0);
            let idx = usize::try_from(gy.saturating_mul(GRID_W).saturating_add(gx)).unwrap_or(0);
            if let Some(cell) = grid.get_mut(idx) {
                *cell = u8::try_from(avg.min(255)).unwrap_or(u8::MAX);
            }
        }
    }
    grid
}

fn pixel_luma(pixel: [u8; 4]) -> u64 {
    (u64::from(pixel[0]) * 299 + u64::from(pixel[1]) * 587 + u64::from(pixel[2]) * 114) / 1_000
}

fn digest_luma(luma: &[u8]) -> String {
    let quantized: Vec<u8> = luma.iter().map(|value| value / 16).collect();
    hex_lower(&Sha256::digest(&quantized))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let high = usize::from(byte >> 4);
        let low = usize::from(byte & 0x0f);
        if let (Some(&h), Some(&l)) = (HEX.get(high), HEX.get(low)) {
            out.push(char::from(h));
            out.push(char::from(l));
        }
    }
    out
}

fn count_changed(prev: &[u8], next: &[u8]) -> usize {
    prev.iter()
        .zip(next.iter())
        .filter(|(a, b)| a.abs_diff(**b) > LUMA_DELTA)
        .count()
}

fn roi_from_diff(
    prev: &[u8],
    next: &[u8],
    width: u32,
    height: u32,
    rgba: &RgbaImage,
) -> Option<Vec<u8>> {
    let mut min_x = GRID_W;
    let mut min_y = GRID_H;
    let mut max_x = 0_u32;
    let mut max_y = 0_u32;
    let mut any = false;
    for gy in 0..GRID_H {
        for gx in 0..GRID_W {
            let idx = usize::try_from(gy.saturating_mul(GRID_W).saturating_add(gx)).unwrap_or(0);
            let Some(a) = prev.get(idx) else {
                continue;
            };
            let Some(b) = next.get(idx) else {
                continue;
            };
            if a.abs_diff(*b) <= LUMA_DELTA {
                continue;
            }
            any = true;
            min_x = min_x.min(gx);
            min_y = min_y.min(gy);
            max_x = max_x.max(gx);
            max_y = max_y.max(gy);
        }
    }
    if !any {
        return None;
    }
    min_x = min_x.saturating_sub(1);
    min_y = min_y.saturating_sub(1);
    max_x = max_x.saturating_add(1).min(GRID_W.saturating_sub(1));
    max_y = max_y.saturating_add(1).min(GRID_H.saturating_sub(1));
    let x0 = min_x.saturating_mul(width) / GRID_W;
    let y0 = min_y.saturating_mul(height) / GRID_H;
    let x1 = (max_x.saturating_add(1).saturating_mul(width) / GRID_W).min(width);
    let y1 = (max_y.saturating_add(1).saturating_mul(height) / GRID_H).min(height);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let crop =
        imageops::crop_imm(rgba, x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0)).to_image();
    let fitted = fit_rgba(&crop, ROI_EDGE);
    encode_png(&fitted).ok()
}

fn fit_rgba(img: &RgbaImage, max_edge: u32) -> RgbaImage {
    let width = img.width().max(1);
    let height = img.height().max(1);
    let edge = width.max(height);
    if edge <= max_edge {
        return img.clone();
    }
    let new_w = ((u64::from(width) * u64::from(max_edge)) / u64::from(edge)).max(1);
    let new_h = ((u64::from(height) * u64::from(max_edge)) / u64::from(edge)).max(1);
    let w = u32::try_from(new_w).unwrap_or(max_edge).max(1);
    let h = u32::try_from(new_h).unwrap_or(max_edge).max(1);
    imageops::thumbnail(img, w, h)
}

fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, ObserveError> {
    let mut out = Cursor::new(Vec::new());
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|_| ObserveError::Encode)?;
    Ok(out.into_inner())
}

/// PNG bytes the vision model should see: overview, optionally stacked with ROI.
///
/// # Errors
///
/// Returns [`ObserveError::Encode`] when the stacked image cannot be written.
pub fn vision_payload(
    overview_png: &[u8],
    roi_png: Option<&[u8]>,
) -> Result<Vec<u8>, ObserveError> {
    let Some(roi) = roi_png else {
        return Ok(overview_png.to_vec());
    };
    let overview = decode_rgba(overview_png)?;
    let roi = decode_rgba(roi)?;
    let width = overview.width().max(roi.width()).max(1);
    let height = overview
        .height()
        .saturating_add(2)
        .saturating_add(roi.height());
    let mut canvas = RgbaImage::new(width, height);
    imageops::replace(&mut canvas, &overview, 0, 0);
    let roi_y = i64::from(overview.height().saturating_add(2));
    imageops::replace(&mut canvas, &roi, 0, roi_y);
    encode_png(&canvas)
}

fn decode_rgba(png: &[u8]) -> Result<RgbaImage, ObserveError> {
    image::load_from_memory(png)
        .map(|img| img.to_rgba8())
        .map_err(|_| ObserveError::InvalidPng)
}

/// Privacy-safe window label for the vision prompt and activity snapshot.
#[must_use]
pub fn observation_send_label(raw_title: &str, settings: &WorldStateSettings) -> String {
    redact_window_title(raw_title, settings.title_mode)
}

/// Whether `title_mode` omits the document title from the model prompt.
#[must_use]
pub fn title_reaches_model(mode: ObservationTitleMode) -> bool {
    !matches!(mode, ObservationTitleMode::AppOnly)
}

/// True when `bytes` look like a PNG or a Base64 screenshot field.
#[must_use]
pub fn contains_raw_screenshot(bytes: &[u8]) -> bool {
    bytes
        .windows(PNG_MAGIC.len())
        .any(|window| window == PNG_MAGIC)
        || bytes.windows(10).any(|window| window == b"png_base64")
}

#[cfg(test)]
pub(crate) fn rgba_png(
    width: u32,
    height: u32,
    fill: [u8; 4],
    paint: impl FnOnce(&mut RgbaImage),
) -> Vec<u8> {
    let mut img = RgbaImage::from_pixel(width, height, image::Rgba(fill));
    paint(&mut img);
    encode_png(&img).expect("test png encodes")
}

#[cfg(test)]
mod tests {
    use super::{ObservationPipeline, ObserveAction, ObserveSkip, rgba_png};
    use image::Rgba;

    fn solid() -> Vec<u8> {
        rgba_png(96, 54, [20, 20, 24, 255], |_| {})
    }

    fn with_block(x: u32, y: u32, color: [u8; 4]) -> Vec<u8> {
        rgba_png(96, 54, [20, 20, 24, 255], |img| {
            for dy in 0..12 {
                for dx in 0..12 {
                    img.put_pixel(x + dx, y + dy, Rgba(color));
                }
            }
        })
    }

    fn caret(on: bool) -> Vec<u8> {
        rgba_png(96, 54, [20, 20, 24, 255], |img| {
            if on {
                for y in 20..36 {
                    img.put_pixel(40, y, Rgba([240, 240, 240, 255]));
                    img.put_pixel(41, y, Rgba([240, 240, 240, 255]));
                }
            }
        })
    }

    #[test]
    fn unchanged_frame_skips_vision_and_reuses_summary() {
        let mut pipe = ObservationPipeline::new();
        let png = solid();
        assert!(matches!(
            pipe.evaluate(&png).unwrap(),
            ObserveAction::Changed { .. }
        ));
        pipe.commit_summary("idle desktop".to_owned());
        let ObserveAction::Skip {
            reason, summary, ..
        } = pipe.evaluate(&png).unwrap()
        else {
            panic!("expected skip");
        };
        assert_eq!(reason, ObserveSkip::Unchanged);
        assert_eq!(summary.as_deref(), Some("idle desktop"));
        assert!(!format!("{pipe:?}").contains("PNG"));
        assert!(!super::contains_raw_screenshot(
            format!("{pipe:?}").as_bytes()
        ));
    }

    #[test]
    fn small_stable_edit_is_detected_after_settle() {
        let mut pipe = ObservationPipeline::new();
        let base = solid();
        assert!(matches!(
            pipe.evaluate(&base).unwrap(),
            ObserveAction::Changed { .. }
        ));
        pipe.commit_summary("before edit".to_owned());
        let speck = rgba_png(96, 54, [20, 20, 24, 255], |img| {
            img.put_pixel(70, 10, Rgba([200, 40, 40, 255]));
            img.put_pixel(71, 10, Rgba([200, 40, 40, 255]));
            img.put_pixel(70, 11, Rgba([200, 40, 40, 255]));
            img.put_pixel(71, 11, Rgba([200, 40, 40, 255]));
        });
        assert!(matches!(
            pipe.evaluate(&speck).unwrap(),
            ObserveAction::Skip {
                reason: ObserveSkip::PendingSmall,
                ..
            }
        ));
        let ObserveAction::Changed { roi_png, .. } = pipe.evaluate(&speck).unwrap() else {
            panic!("settled tiny edit should summarize");
        };
        assert!(roi_png.is_some());
    }

    #[test]
    fn meaningful_edit_emits_roi_immediately() {
        let mut pipe = ObservationPipeline::new();
        let base = solid();
        assert!(matches!(
            pipe.evaluate(&base).unwrap(),
            ObserveAction::Changed { .. }
        ));
        pipe.commit_summary("before edit".to_owned());
        let edited = with_block(60, 8, [200, 40, 40, 255]);
        let ObserveAction::Changed {
            roi_png,
            roi_composited,
            ..
        } = pipe.evaluate(&edited).unwrap()
        else {
            panic!("block edit should summarize with ROI");
        };
        assert!(roi_png.is_some());
        assert!(roi_composited);
    }

    #[test]
    fn caret_blink_is_suppressed() {
        let mut pipe = ObservationPipeline::new();
        let off = caret(false);
        let on = caret(true);
        assert!(matches!(
            pipe.evaluate(&off).unwrap(),
            ObserveAction::Changed { .. }
        ));
        pipe.commit_summary("editor".to_owned());
        let _first_blink = pipe.evaluate(&on).unwrap();
        let ObserveAction::Skip {
            reason, summary, ..
        } = pipe.evaluate(&off).unwrap()
        else {
            panic!("caret should skip vision");
        };
        assert!(matches!(
            reason,
            ObserveSkip::CaretBlink | ObserveSkip::PendingSmall | ObserveSkip::Clock
        ));
        assert_eq!(summary.as_deref(), Some("editor"));
        assert!(matches!(
            pipe.evaluate(&on).unwrap(),
            ObserveAction::Skip { .. }
        ));
    }
}
