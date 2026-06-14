//! # ene-desktop-v2
//!
//! Winit + wgpu shell for the ene AI character platform.
//!
//! PR0 (shipped) wired the transparent-window recipe and a
//! single-window red-rect smoke test.
//!
//! PR1 (this revision) layers on the runtime skeleton needed for
//! the rest of the migration:
//!
//! - tokio multi-thread runtime (for [`EneHandle`](ene_core::EneHandle))
//! - [`AiBridge`] forwarding AI events into the cross-subsystem bus
//! - [`TrayHandle`] for the system tray (Win32 message thread / GTK pump)
//! - [`CharacterSettings`] plain-struct analog of the legacy Bevy resource
//! - CLI argument parsing for VRM/VRMA overrides
//! - cross-subsystem [`AppEvent`] bus drained by the winit runtime
//!
//! See `docs/architecture/wgpu-migration.md` §22.3 (PR0) and §22
//! (full roadmap) for context.
mod ai_bridge;
mod character;
mod character_state;
mod events;
mod gpu;
mod look_at;
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

    // Build the tokio multi-thread runtime; keep its `enter()` guard
    // alive for the lifetime of `main` so that `AiBridge::new` (and
    // any other subsystem) can `tokio::spawn` from synchronous
    // contexts.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let handle = runtime.handle().clone();
    let _guard = runtime.enter();

    // Resolve assets + CLI overrides, then build the GpuContext and
    // the cross-subsystem AppState.
    let (assets_dir, default_vrm, _default_vrma) = state::resolve_paths()?;
    tracing::info!(
        "ene-desktop-v2 starting: assets={}, default_vrm={}",
        assets_dir.display(),
        default_vrm
    );

    let gpu = pollster::block_on(gpu::GpuContext::new())?;
    let settings = settings::CharacterSettings::discover(&assets_dir, default_vrm);
    let (app_state, event_tx) = state::AppState::with_channel(gpu, settings, &handle);

    let event_loop = winit::event_loop::EventLoop::new()?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);

    let mut app = runtime::Runtime::new(app_state, event_tx);
    event_loop
        .run_app(&mut app)
        .expect("winit event loop failed");

    // PR4.2 fix: shut the tokio runtime down gracefully so the
    // AI-bridge background tasks (`pump_events` + the bootstrap
    // `load_config` / `load_character` task) get a chance to exit
    // cleanly. Without this, `runtime` is dropped at the end of
    // `main`, the runtime cancels in-flight tasks, and any
    // `tokio::time::Timeout` future inside `EneHandle::load_config`
    // panics on drop with "A Tokio 1.x context was found, but it
    // is being shutdown." A 500 ms budget is enough for the actor
    // to drain its broadcast receiver and the bootstrap task to
    // finish / log its warn.
    drop(_guard);
    runtime.shutdown_timeout(std::time::Duration::from_millis(500));

    Ok(())
}
