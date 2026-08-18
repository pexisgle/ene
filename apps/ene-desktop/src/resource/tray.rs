//! `tray_icon::TrayIcon` is `Rc<RefCell<_>>` and therefore not
//! `Send + Sync`, so it cannot be a bevy `Resource`. The tray
//! handle stays on `AppState::tray`; the `Messages<TickGtk>` queue
//! is drained from `runtime.rs`.
#[cfg(target_os = "linux")]
use bevy_ecs::prelude::*;

/// Linux-only flag flipped to `true` once [`gtk::init`] has
/// succeeded.
///
/// `gtk::events_pending()` panics with
/// `"GTK has not been initialized. Call gtk::init first."`
/// if it is called before [`gtk::init`]. The pump systems in
/// [`crate::system::platform::gtk_pump`] and
/// [`crate::system::tray_tick`] therefore read this resource
/// and early-return while GTK is not yet initialised.
#[cfg(target_os = "linux")]
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct GtkReady(pub bool);
