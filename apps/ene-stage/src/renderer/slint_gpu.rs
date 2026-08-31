//! Isolation boundary for Slint's `unstable-wgpu-29` renderer.
//!
//! Production overlay UI is rendered offscreen into an `Rgba8Unorm` texture
//! owned by [`super::StageRenderer`]. This module never reads that texture
//! back to the CPU and never copies it with `copy_texture_to_texture`;
//! [`super::compositor`] composites with a premul fullscreen triangle.

use wgpu::TextureView;

/// Offscreen Slint overlay for one stage frame.
///
/// A `None` layer (or `render` returning `false`) skips the compositor pass
/// so VRM-only frames stay on the existing overlay path.
pub struct SlintOverlayLayer {
    size: (u32, u32),
}

impl SlintOverlayLayer {
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self {
            size: (width, height),
        }
    }

    #[must_use]
    pub const fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Draw the current overlay UI into `target`.
    ///
    /// Returns `true` when any pixels were written. The stub always returns
    /// `false` until the Slint wgpu adapter is wired (issue 1265).
    pub fn render(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _target: &TextureView,
    ) -> bool {
        let _ = self.size;
        false
    }
}
