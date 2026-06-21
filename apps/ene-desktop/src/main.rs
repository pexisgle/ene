//! # ene-desktop
//!
//! Winit + wgpu shell for the ene AI character platform. Owns the
//! `AiBridge`, system tray, character renderer, and the cross-subsystem
//! [`AppEvent`] bus.
mod ai_bridge;
mod character;
mod character_state;
mod events;
mod gpu;
#[cfg(target_os = "linux")]
mod input_region_debug;
mod look_at;
#[cfg(target_os = "linux")]
mod mask_gizmo;
mod physics;
#[cfg(target_os = "linux")]
mod platform;
mod raycast_debug;
mod runtime;
mod settings;
mod settings_ui;
mod state;
mod tray;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"));
    fmt().with_env_filter(filter).init();

    // The enter guard must live for the rest of `main` so that
    // `AiBridge::new` (and any other subsystem) can `tokio::spawn`
    // from synchronous contexts.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let handle = runtime.handle().clone();
    let _guard = runtime.enter();

    let (assets_dir, default_vrm, _default_vrma) = state::resolve_paths()?;
    tracing::info!(
        "ene-desktop starting: assets={}, default_vrm={}",
        assets_dir.display(),
        default_vrm
    );

    let gpu = pollster::block_on(gpu::GpuContext::new())?;
    let mut settings = settings::CharacterSettings::discover(&assets_dir, default_vrm);
    // Reset parked position on launch: an earlier debug run can mutate
    // `character_position` and leave the model off-screen until the
    // drag-to-move UI provides a "Reset position" affordance.
    settings.character_state.character_position = glam::Vec3::ZERO;
    settings.save();
    let (app_state, event_tx) = state::AppState::with_channel(gpu, settings, &handle);

    let event_loop = winit::event_loop::EventLoop::new()?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);

    let mut app = runtime::Runtime::new(app_state, event_tx);
    event_loop
        .run_app(&mut app)
        .expect("winit event loop failed");

    // Shut the tokio runtime down gracefully so background tasks
    // (AI bridge pump, bootstrap load) can exit cleanly. Dropping
    // `runtime` outright cancels them and any pending
    // `tokio::time::Timeout` future panics on drop.
    drop(_guard);
    runtime.shutdown_timeout(std::time::Duration::from_millis(500));

    Ok(())
}
