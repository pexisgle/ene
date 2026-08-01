//! Screen-diff gate that skips redundant vision inference when the screen
//! content has not changed significantly.
//!
//! Uses a simple average-hash (aHash) comparison on the downscaled screen
//! capture. When the Hamming distance between successive hashes is below a
//! threshold the gate returns the cached summary instead of triggering a new
//! inference call.

use image::DynamicImage;

/// Perceptual hash gate that caches the last screen hash and summary.
///
/// # Usage
///
/// ```ignore
/// let mut gate = ScreenDiffGate::new();
///
/// // On every observation tick:
/// let hash = average_hash(&captured_image);
/// if let Some(cached) = gate.check(hash) {
///     // Screen hasn't changed much — reuse previous summary.
///     return cached;
/// }
///
/// // … run expensive vision inference …
/// gate.cache(hash, new_summary);
/// ```
pub struct ScreenDiffGate {
    last_hash: Option<u64>,
    last_summary: Option<String>,
}

impl ScreenDiffGate {
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_hash: None,
            last_summary: None,
        }
    }

    /// Check whether the screen has changed significantly since the last
    /// [`Self::cache`] call.
    ///
    /// Returns `Some(cached_summary)` when the Hamming distance between
    /// `current_hash` and the last cached hash is at most 5 (the default
    /// threshold). Returns `None` when re-inference is needed.
    #[must_use]
    pub fn check(&self, current_hash: u64) -> Option<&str> {
        let last = self.last_hash?;
        let distance = (current_hash ^ last).count_ones();
        if distance <= 5 {
            return self.last_summary.as_deref();
        }
        None
    }

    pub fn cache(&mut self, hash: u64, summary: String) {
        self.last_hash = Some(hash);
        self.last_summary = Some(summary);
    }
}

impl Default for ScreenDiffGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute a simple **average hash** for an image.
///
/// 1. Downscale to 8×8 grayscale (Lanczos3).
/// 2. Compute the mean pixel value.
/// 3. Build a 64-bit hash where each bit is `1` if the pixel is above average.
///
/// This is a lightweight perceptual hash suitable for detecting
/// frame-to-frame changes in screen captures.
pub fn average_hash(img: &DynamicImage) -> u64 {
    let small = img.resize_exact(8, 8, image::imageops::FilterType::Lanczos3);
    let gray = small.to_luma8();
    let pixels: Vec<u8> = gray.pixels().map(|p| p[0]).collect();
    let avg = pixels.iter().map(|&x| u64::from(x)).sum::<u64>() / 64;
    let mut hash: u64 = 0;
    for (i, &p) in pixels.iter().enumerate() {
        if u64::from(p) > avg {
            hash |= 1 << i;
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_images_produce_same_hash() {
        let img = DynamicImage::new_rgba8(100, 100);
        let h1 = average_hash(&img);
        let h2 = average_hash(&img);
        assert_eq!(h1, h2);
    }

    #[test]
    fn different_images_have_different_hashes() {
        // A uniform image always hashes to 0 (no pixel exceeds the mean), so
        // we use images with spatial variation to exercise the hash properly.
        let mut dark = DynamicImage::new_rgba8(100, 100);
        let mut bright = DynamicImage::new_rgba8(100, 100);
        // Dark: left half black, right half mid-grey.
        // Bright: left half white, right half mid-grey.
        for y in 0..100u32 {
            for x in 0..100u32 {
                let px = dark.as_mut_rgba8().unwrap().get_pixel_mut(x, y);
                let v = if x < 50 { 0 } else { 128 };
                *px = image::Rgba([v, v, v, 255]);

                let px2 = bright.as_mut_rgba8().unwrap().get_pixel_mut(x, y);
                let v2 = if x < 50 { 255 } else { 128 };
                *px2 = image::Rgba([v2, v2, v2, 255]);
            }
        }
        let h1 = average_hash(&dark);
        let h2 = average_hash(&bright);
        assert_ne!(
            h1, h2,
            "spatially different images should produce different hashes"
        );
    }

    #[test]
    fn gate_returns_none_when_empty() {
        let gate = ScreenDiffGate::new();
        assert!(gate.check(42).is_none());
    }

    #[test]
    fn gate_returns_cached_when_similar() {
        let mut gate = ScreenDiffGate::new();
        gate.cache(0xAAAA_AAAA_AAAA_AAAA, "hello".into());
        // Single bit flip — still within the default threshold of 5.
        let similar = 0xAAAA_AAAA_AAAA_AAAB;
        assert_eq!(gate.check(similar), Some("hello"));
    }

    #[test]
    fn gate_returns_none_when_different() {
        let mut gate = ScreenDiffGate::new();
        gate.cache(0x0000_0000_0000_0000, "hello".into());
        // All bits flipped — distance of 64 which exceeds the threshold.
        let different = 0xFFFF_FFFF_FFFF_FFFF;
        assert!(gate.check(different).is_none());
    }
}
