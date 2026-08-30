//! Isolated Stage UI probes for Slint + shared wgpu.
//!
//! Not a product path. The production `ene-stage` binary is unchanged.

#![expect(
    clippy::print_stdout,
    reason = "PoC measurements and hit logs go to stdout"
)]

pub mod app;
pub mod blit;
pub mod exp_c;
pub mod exp_d;
pub mod exp_d2;
pub mod gpu;
pub mod input;
pub mod metrics;
pub mod os_input;
pub mod region;
pub mod slint_host;
pub mod triangle;
pub mod vrm_scene;
pub mod x11_split;

pub use app::{PocMode, run};
pub use input::{
    PointerTarget, ScreenPoint, ScreenRect, VrmHitLayout, VrmPart, interactive_rects, route_pointer,
};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PocError {
    #[error("gpu: {0}")]
    Gpu(#[from] gpu::GpuError),
    #[error("window: {0}")]
    Window(String),
    #[error("slint: {0}")]
    Slint(String),
    #[error("surface: {0}")]
    Surface(String),
}

pub fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("ene_stage_poc=info,info"));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
    drop(subscriber);
}

#[cfg(test)]
mod tests {
    #[test]
    fn composition_path_does_not_read_back_gpu_textures() {
        let app = include_str!("app.rs");
        let blit = include_str!("blit.rs");
        let triangle = include_str!("triangle.rs");
        let vrm = include_str!("vrm_scene.rs");
        let exp_c = include_str!("exp_c.rs");
        let exp_d2 = include_str!("exp_d2.rs");
        for src in [app, blit, triangle, vrm, exp_c, exp_d2] {
            assert!(
                !src.contains("texture.map_async") && !src.contains("Buffer::map_async"),
                "GPU→CPU map is forbidden in the PoC"
            );
            assert!(
                !src.contains("copy_texture_to_buffer("),
                "GPU→CPU texture readback is forbidden in the PoC"
            );
            assert!(
                !src.contains("copy_texture_to_texture("),
                "composition uses a render pass, not copy_texture_to_texture"
            );
            assert!(
                !src.contains("SharedPixelBuffer"),
                "CPU pixel buffers are not used for composition"
            );
        }
        assert!(
            blit.contains("PREMULTIPLIED_ALPHA_BLENDING") && blit.contains("LoadOp::Load"),
            "UI composite must be a premul fullscreen pass that loads the VRM target"
        );
    }
}
