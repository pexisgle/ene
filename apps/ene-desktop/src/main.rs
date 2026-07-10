//! # ene-desktop
//!
//! Winit + wgpu shell for the ene AI character platform. Owns the
//! `AiBridge`, system tray, character renderer, and the cross-subsystem
//! [`AppEvent`] bus.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod acquire_error;
mod ai_bridge;
mod app;
mod character;
mod character_state;
mod chat_state;
mod chat_ui;
mod component;
mod event;
mod events;
mod gpu;
mod i18n;
#[cfg(target_os = "linux")]
mod input_region_debug;
mod look_at;
#[cfg(target_os = "linux")]
mod mask_gizmo;
mod memory_journal;
mod physics;
#[cfg(target_os = "linux")]
mod platform;
mod plugin;
mod raycast_debug;
mod resource;
mod runtime;
mod schedule;
mod settings;
mod settings_ui;
mod state;
mod system;
mod tray;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("info,sqlx=warn,sea_orm=warn,wgpu_core=warn,wgpu_hal=warn,naga=warn")
    });
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
    let settings = settings::CharacterSettings::discover(&assets_dir, default_vrm);
    let (app_state, event_tx) = state::AppState::with_channel(gpu, settings, &handle);

    let event_loop = winit::event_loop::EventLoop::new()?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);

    let mut app = runtime::Runtime::new(app_state, event_tx);
    event_loop.run_app(&mut app)?;

    // Shut the tokio runtime down gracefully so background tasks
    // (AI bridge pump, bootstrap load) can exit cleanly. Dropping
    // `runtime` outright cancels them and any pending
    // `tokio::time::Timeout` future panics on drop.
    drop(_guard);
    runtime.shutdown_timeout(std::time::Duration::from_millis(500));

    Ok(())
}
