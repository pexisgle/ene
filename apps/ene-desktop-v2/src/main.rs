//! # ene-desktop-v2 (minimum scaffold)
//!
//! winit + wgpu foundation for the ene AI character platform.
//!
//! PR0 (this crate's scope) is a runnable smoke test for the
//! transparency recipe that `apps/tw-test` proved out:
//!
//! 1. Open a transparent, always-on-top, decoration-less window via
//!    winit + wgpu 27 with `CompositeAlphaMode::PreMultiplied`.
//! 2. Draw a single red rectangle so we can visually confirm the
//!    surface is being presented.
//! 3. `Space` toggles transparency (off: gray + title bar; on:
//!    transparent + no decorations).
//! 4. `Escape` or the window's close button exits cleanly.
//!
//! See `docs/architecture/wgpu-migration.md` §22.3 for context and
//! §4 for the larger PR roadmap (tray, AI bridge, egui, VRM, ...).
mod gpu;
mod runtime;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"));
    fmt().with_env_filter(filter).init();

    let gpu = pollster::block_on(gpu::GpuContext::new())?;

    let event_loop = winit::event_loop::EventLoop::new()?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);

    let mut runtime = runtime::Runtime::new(gpu);
    event_loop
        .run_app(&mut runtime)
        .expect("winit event loop failed");
    Ok(())
}
