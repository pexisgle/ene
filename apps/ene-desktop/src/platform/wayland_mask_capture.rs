//! PR-LX.3: Wayland mask capture (offscreen R8 + readback).
//!
//! The Linux click-through story on Wayland needs the *visible
//! silhouette* of the character as a set of rectangles, so the
//! runtime can push them into
//! [`crate::platform::wayland_region::WaylandInputRegionContext`]
//! via `wl_region::add` and `wl_surface::set_input_region`. We
//! can't read the actual winit swapchain (winit owns the surface
//! and does not expose it for readback from a stand-alone
//! connection), so we render a separate, low-resolution copy of
//! the model into an offscreen `Rgba8Unorm` texture and read it
//! back once per frame.
//!
//! The target texture is sized at
//! `(width / mask_render_downsample, height / mask_render_downsample)`
//! — the `mask_render_downsample` value comes from
//! `CharacterSettings::graphics::mask_render_downsample` and
//! defaults to `8` (matching the legacy `apps/ene-desktop`.
//! `cycle_mask_render_downsample` UI row). At 560×980 with
//! downsample=8 the texture is 70×122, which is small enough for
//! the readback to cost ~3.4 KB and the rectangle extractor to run
//! in a single pass over the buffer.
//!
//! The actual render pass that writes into this target is wired
//! up in PR-LX.7 (the runtime's Linux dispatch). This module
//! owns the GPU resources + the CPU-side rectangle extractor
//! and is `cfg(target_os = "linux")`-only.

use std::sync::Arc;

/// Format used for the mask capture target. `Rgba8Unorm` is the
/// lowest-bandwidth format the wgpu spec mandates for
/// `RENDER_ATTACHMENT | COPY_SRC`, and our rectangle extractor
/// only reads the red channel anyway. The four-channel layout is
/// a wgpu-validation convenience: a pure `R8Unorm` target works
/// too, but wgpu's strictest validation rules occasionally
/// complain when COPY_SRC / RENDER_ATTACHMENT are combined on
/// the single-channel formats; the bandwidth difference is
/// negligible at downsample=8.
pub const MASK_TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Bytes per pixel of [`MASK_TARGET_FORMAT`]. Used to slice the
/// readback buffer.
const BYTES_PER_PIXEL: u64 = 4;

/// Round `width * BYTES_PER_PIXEL` up to the next multiple of
/// [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`] (256 in wgpu 29).
/// wgpu validation requires `TexelCopyBufferInfo::bytes_per_row`
/// to be a multiple of that constant for any `copy_texture_to_buffer`
/// that crosses a row boundary.
///
/// The matching staging buffer must be sized at
/// `bytes_per_row * height`; the trailing per-row padding bytes
/// are unread by the CPU (`extract_rectangles` walks per-pixel
/// using `width`, not the stride).
pub(crate) const fn aligned_bytes_per_row(width: u32) -> u32 {
    (width as u64 * BYTES_PER_PIXEL).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64)
        as u32
}

/// Threshold above which a pixel is considered "inside the
/// silhouette" (we use the red channel of the captured
/// `Rgba8Unorm` target). The mask shader writes
/// `(1.0, 1.0, 1.0, 1.0)` for any fragment covered by a mesh
/// triangle, so 16/255 = 6% is plenty of headroom against
/// edge-fuzz from the wgpu primitive rasteriser.
const PIXEL_THRESHOLD: u8 = 16;

/// Offscreen mask capture target. Holds the texture, view, and
/// readback buffer for a single, downsampled silhouette read.
///
/// Construction is cheap: two GPU resources + a single 4 KB
/// staging buffer at default downsample=8. The actual render
/// pass that writes into `target_view()` lives in PR-LX.7 (the
/// runtime's Linux dispatch); this module only allocates the
/// target and exposes the rectangle-extraction readback.
#[allow(dead_code)] // PR-LX.7 wires the mask render pass; the public API is fixed here.
pub struct MaskCaptureCamera {
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    readback: wgpu::Buffer,
    /// `width / mask_render_downsample` (post-clamp).
    width: u32,
    /// `height / mask_render_downsample` (post-clamp).
    height: u32,
    /// Cached `bytes_per_row` for the readback. This is
    /// `aligned_bytes_per_row(width)`: the unpadded
    /// `width * BYTES_PER_PIXEL` rounded up to a multiple of
    /// [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`] (256 in wgpu 29).
    /// The CPU walk in `extract_rectangles` indexes per-pixel
    /// using `width`, so the trailing per-row padding bytes
    /// are unread.
    bytes_per_row: u32,
    /// Cached downsample so we can detect size changes.
    downsample: u32,
    /// Cached readback bytes; populated by
    /// `read_back` and consumed by `extract_rectangles`.
    last_readback: Vec<u8>,
    /// `true` once `read_back` has been called at least once for
    /// the current texture contents. Until then
    /// `extract_rectangles` returns the previous frame's
    /// rectangles (or an empty vec on the very first frame).
    readback_fresh: bool,
    /// True if the buffer is currently mapped (via `map_async`).
    /// `encode_readback` skips the copy to prevent wgpu validation errors.
    pub is_mapped: bool,
}

#[allow(dead_code)] // PR-LX.7 wires the mask render pass; the public API is fixed here.
impl MaskCaptureCamera {
    /// Build a new mask capture target for the given window
    /// size + downsample. `downsample` is clamped to at least
    /// `1`; a `downsample` of `0` would divide by zero.
    #[allow(dead_code)] // PR-LX.7 wires the mask render pass; the public API is fixed here.
    pub fn try_new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        downsample: u32,
    ) -> Option<Self> {
        let downsample = downsample.max(1);
        let (w, h) = downsample_dims(width, height, downsample);
        Self::try_new_sized(device, w, h, downsample)
    }

    /// Build a new mask capture target with explicit
    /// (downsampled) dimensions. `downsample` is stored for
    /// [`Self::resize`] comparisons.
    fn try_new_sized(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        downsample: u32,
    ) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }

        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mask.target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MASK_TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

        // wgpu 29 requires `TexelCopyBufferInfo::bytes_per_row`
        // to be a multiple of `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`.
        // Round `width * BYTES_PER_PIXEL` up to the next
        // multiple and size the staging buffer accordingly.
        let bytes_per_row = aligned_bytes_per_row(width);
        let readback_size = (bytes_per_row as u64) * (height as u64);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mask.readback"),
            size: readback_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Some(Self {
            target,
            target_view,
            readback,
            width,
            height,
            bytes_per_row,
            downsample,
            // The CPU cache matches the staging buffer so
            // `read_back`'s `copy_from_slice` is a 1:1 copy.
            // The trailing per-row padding bytes (if any) are
            // stored but never inspected by
            // `extract_rectangles`.
            last_readback: vec![0u8; readback_size as usize],
            readback_fresh: false,
            is_mapped: false,
        })
    }

    /// Resize the target to match a new window size /
    /// `mask_render_downsample`. Re-creates the texture +
    /// readback buffer. Returns `true` if anything changed
    /// (texture, buffer, or cached dimensions) and `false` if
    /// the new size is identical.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        window_width: u32,
        window_height: u32,
        downsample: u32,
    ) -> bool {
        let downsample = downsample.max(1);
        let (w, h) = downsample_dims(window_width, window_height, downsample);
        if w == self.width && h == self.height && downsample == self.downsample {
            return false;
        }
        if let Some(replacement) = Self::try_new_sized(device, w, h, downsample) {
            *self = replacement;
            true
        } else {
            // Zero / negative size — leave the camera at its
            // previous dimensions; the runtime will skip
            // capture this frame.
            false
        }
    }

    /// View into which the mask shader should write. The
    /// runtime (PR-LX.7) renders the model into this view once
    /// per frame.
    pub fn target_view(&self) -> &wgpu::TextureView {
        &self.target_view
    }

    /// Width of the mask target in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height of the mask target in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Downsample factor (`window_pixels / target_pixels`).
    /// `extract_rectangles` returns rectangles in
    /// **target** pixel space; the runtime scales them by
    /// this factor to convert back to window-pixel space
    /// before pushing to Wayland / X11.
    pub fn downsample(&self) -> u32 {
        self.downsample
    }

    /// Append a `copy_texture_to_buffer` command to the
    /// encoder. The runtime calls this *after* the mask
    /// render pass and *before* `queue.submit`. The matching
    /// `read_back` happens at the start of the next frame
    /// (one-frame latency is fine; the cursor moves faster
    /// than 60 Hz but the silhouette changes on the
    /// sub-second timescale of the animation).
    pub fn encode_readback(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.is_mapped {
            return;
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Block on the GPU's readback of the most recently
    /// submitted encoder and refresh `last_readback`. The
    /// runtime calls this once per frame, after
    /// `queue.submit` and before the rectangle extractor
    /// runs.
    ///
    /// We use a blocking `mpsc::recv` because the mask
    /// readback is a tiny, single-frame job — the wait is
    /// bounded by the swapchain interval and the typical
    /// latency is sub-millisecond. The legacy Bevy app
    /// used the same pattern in `apps/ene-desktop/src/
    /// character_drag/linux/capture.rs::read_mask`
    /// (PR5.3 plan, §10.3).
    ///
    /// **LX.9 deprecation note:** the production runtime
    /// no longer calls `read_back` on the winit main thread
    /// (a blocking `mpsc::recv` froze the window — see
    /// `mask_readback` module for the off-thread worker).
    /// The method is retained under `#[cfg(test)]` so the
    /// GPU writeback logic still has a one-call entry point
    /// for synthetic tests.
    #[cfg(test)]
    pub fn read_back(&mut self, _device: &wgpu::Device, _queue: &wgpu::Queue) {
        let slice = self.readback_slice();
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = rx.recv();
        let mapped_len = self.copy_mapped_into_last_readback();
        let _ = mapped_len;
        self.unmap_readback();
    }

    /// Returns a `BufferSlice` over the readback staging
    /// buffer. The mask readback worker uses this in
    /// `map_async`; the slice is `wgpu::Buffer::slice` is
    /// `&self` so no mutability is required.
    pub fn readback_slice(&mut self) -> wgpu::BufferSlice<'_> {
        self.is_mapped = true;
        self.readback.slice(..)
    }

    /// Copy the bytes currently mapped by the GPU into
    /// `self.last_readback` and refresh `readback_fresh`.
    /// Returns the number of bytes copied, or `0` if the
    /// buffer is not currently mapped.
    pub fn copy_mapped_into_last_readback(&mut self) -> usize {
        let mapped = self.readback.slice(..).get_mapped_range();
        let len = mapped.len();
        if len == 0 {
            return 0;
        }
        self.last_readback.resize(len, 0u8);
        self.last_readback.copy_from_slice(&mapped);
        self.readback_fresh = true;
        len
    }

    /// Unmap the readback staging buffer. No-op if the
    /// buffer is not currently mapped.
    pub fn unmap_readback(&mut self) {
        if self.is_mapped {
            self.readback.unmap();
            self.is_mapped = false;
        }
    }

    /// Decompose the most recent readback into a list of
    /// `(x, y, w, h)` rectangles in the **downsampled**
    /// coordinate space. Returns the cached rectangles from
    /// the previous frame if no new readback is available
    /// yet (or an empty vec on the very first frame).
    pub fn extract_rectangles(&self) -> Vec<(i32, i32, i32, i32)> {
        extract_rectangles_impl(
            self.width,
            self.height,
            self.bytes_per_row,
            &self.last_readback,
            self.readback_fresh,
        )
    }
}

fn extract_rectangles_impl(
    width: u32,
    height: u32,
    bytes_per_row: u32,
    last_readback: &[u8],
    readback_fresh: bool,
) -> Vec<(i32, i32, i32, i32)> {
    if !readback_fresh {
        return Vec::new();
    }
    let mut rects = Vec::new();
    let stride = width as usize * BYTES_PER_PIXEL as usize;
    for y in 0..height as usize {
        let row_start = y * bytes_per_row as usize;
        if row_start + stride > last_readback.len() {
            break;
        }
        let row = &last_readback[row_start..row_start + stride];
        let mut x: usize = 0;
        while x < width as usize {
            let r = row[x * BYTES_PER_PIXEL as usize];
            if r <= PIXEL_THRESHOLD {
                x += 1;
                continue;
            }
            let start = x;
            while x < width as usize && row[x * BYTES_PER_PIXEL as usize] > PIXEL_THRESHOLD {
                x += 1;
            }
            rects.push((start as i32, y as i32, (x - start) as i32, 1));
        }
    }
    rects
}

/// Build a mask capture camera in an `Arc<Mutex<…>>` so the
/// runtime can hand a clone to the click-through dispatcher
/// while still keeping the `read_back` / `extract_rectangles`
/// path single-threaded. The legacy Bevy app used the same
/// pattern in `apps/ene-desktop/src/character_drag/linux/
/// capture.rs::MaskCaptureState` (PR5.3 plan, §13.2).
#[allow(dead_code)] // PR-LX.7 wires the mask render pass; the public API is fixed here.
pub type MaskCaptureState = Arc<parking_lot::Mutex<MaskCaptureCamera>>;

/// Build a fresh `MaskCaptureState` for the given window size
/// and downsample, returning `None` if the size is zero (e.g.
/// during early startup).
#[allow(dead_code)] // PR-LX.7 wires the mask render pass; the public API is fixed here.
pub fn new_mask_capture_state(
    device: &wgpu::Device,
    window_width: u32,
    window_height: u32,
    downsample: u32,
) -> Option<MaskCaptureState> {
    MaskCaptureCamera::try_new(device, window_width, window_height, downsample)
        .map(|c| Arc::new(parking_lot::Mutex::new(c)))
}

/// Compute the downsampled (width, height) pair. Clamps to a
/// minimum of 1×1 so the texture descriptor is always valid
/// even when the window is briefly zero-sized during resize.
fn downsample_dims(width: u32, height: u32, downsample: u32) -> (u32, u32) {
    let d = downsample.max(1);
    let w = (width / d).max(1);
    let h = (height / d).max(1);
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsample_dims_zero_window_size_returns_one() {
        assert_eq!(downsample_dims(0, 0, 8), (1, 1));
        assert_eq!(downsample_dims(0, 100, 4), (1, 25));
        assert_eq!(downsample_dims(100, 0, 4), (25, 1));
    }

    #[test]
    fn downsample_dims_normal_size_rounds_down() {
        // 560 / 8 = 70, 980 / 8 = 122 (legacy default)
        assert_eq!(downsample_dims(560, 980, 8), (70, 122));
        // 1 = identity
        assert_eq!(downsample_dims(560, 980, 1), (560, 980));
        // divisor 4
        assert_eq!(downsample_dims(560, 980, 4), (140, 245));
    }

    #[test]
    fn downsample_dims_zero_downsample_clamped_to_one() {
        assert_eq!(downsample_dims(100, 100, 0), (100, 100));
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn threshold_filters_low_alpha() {
        // 17 → inside (>, strict), 15 → outside (≤).
        // The extractor uses `r > PIXEL_THRESHOLD` to
        // flag a pixel as inside the silhouette, with
        // `PIXEL_THRESHOLD = 16`.
        assert!(PIXEL_THRESHOLD > 0);
        assert!(17 > PIXEL_THRESHOLD);
        assert!(15 <= PIXEL_THRESHOLD);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn mask_target_format_is_rgba8unorm() {
        // Pin the format choice: changing it requires
        // changing `BYTES_PER_PIXEL` too, and we want the
        // compiler to fail loudly if someone updates one
        // without the other.
        assert_eq!(MASK_TARGET_FORMAT, wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(BYTES_PER_PIXEL, 4);
    }

    #[test]
    fn bytes_per_row_is_aligned_to_256() {
        // wgpu 29 validation: `bytes_per_row` must be a
        // multiple of `COPY_BYTES_PER_ROW_ALIGNMENT` (256).
        // The 4bpp mask is small so the unpadded
        // `width * 4` is often not a multiple of 256.
        assert_eq!(aligned_bytes_per_row(70) % 256, 0);
        assert_eq!(aligned_bytes_per_row(71) % 256, 0);
        assert_eq!(aligned_bytes_per_row(1) % 256, 0);
        // Sanity: the rounded value is at least the
        // unpadded width.
        assert!(aligned_bytes_per_row(70) >= 70 * 4);
    }

    #[test]
    fn bytes_per_row_alignment_typical_window() {
        // 560 / 8 = 70, the typical legacy default window
        // size at downsample 8 → 70 px wide mask. 70 * 4
        // = 280 is already a multiple of 256 (next
        // multiple: 512). The helper must still return
        // 512.
        assert_eq!(aligned_bytes_per_row(70), 512);
        // 71 * 4 = 284, next multiple of 256 = 512.
        assert_eq!(aligned_bytes_per_row(71), 512);
        // 64 * 4 = 256, exact.
        assert_eq!(aligned_bytes_per_row(64), 256);
    }

    #[test]
    fn extract_rectangles_respects_row_stride_alignment() {
        let width = 70;
        let height = 2;
        let bytes_per_row = aligned_bytes_per_row(width); // 512
        assert_eq!(bytes_per_row, 512);

        let mut data = vec![0u8; (bytes_per_row * height) as usize];

        // Row 0, pixel index 10 (offset 10 * 4 = 40)
        data[40] = 255;
        // Row 1, pixel index 20 (offset 512 + 20 * 4 = 592)
        data[(bytes_per_row as usize) + 80] = 255;

        let rects = extract_rectangles_impl(width, height, bytes_per_row, &data, true);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0], (10, 0, 1, 1));
        assert_eq!(rects[1], (20, 1, 1, 1));
    }
}
