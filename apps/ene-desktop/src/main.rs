//! # ene-desktop
//!
//! Winit + wgpu shell for the ene AI character platform. Owns the
//! `AiBridge`, system tray, character renderer, and the cross-subsystem
//! [`AppEvent`] bus.
#![expect(
    clippy::option_if_let_else,
    clippy::unused_self,
    clippy::needless_pass_by_ref_mut,
    clippy::collapsible_match,
    clippy::match_same_arms,
    clippy::significant_drop_tightening,
    clippy::branches_sharing_code,
    clippy::needless_pass_by_value,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::panic,
    clippy::unnecessary_wraps,
    reason = "desktop UI/render loop favors local clarity; graphics math uses intentional arithmetic"
)]
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "unit/integration tests use unwrap/expect for assertions"
    )
)]

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
mod proactive_observe;
mod raycast_debug;
mod resource;
mod runtime;
mod schedule;
mod settings;
mod settings_ui;
mod startup;
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
    let guard = runtime.enter();

    let paths = startup::first_launch_setup()?;
    tracing::info!(
        "ene-desktop starting: assets={}, default_vrm={}",
        paths.assets_dir.display(),
        paths.default_vrm
    );

    let gpu = pollster::block_on(gpu::GpuContext::new())?;
    let settings = startup::load_desktop_settings(&paths);
    let (app_state, event_tx) = startup::init_app_state(gpu, settings, &handle);

    let event_loop = winit::event_loop::EventLoop::new()?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);

    let mut app = runtime::Runtime::new(app_state, event_tx);
    event_loop.run_app(&mut app)?;

    // Shut the tokio runtime down gracefully so background tasks
    // (AI bridge pump, bootstrap load) can exit cleanly. Dropping
    // `runtime` outright cancels them and any pending
    // `tokio::time::Timeout` future panics on drop.
    drop(guard);
    runtime.shutdown_timeout(std::time::Duration::from_millis(500));

    Ok(())
}
