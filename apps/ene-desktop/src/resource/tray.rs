//! Linux-only tray handle plumbing.
//!
//! `tray_icon::TrayIcon` is `Rc<RefCell<_>>` and therefore not
//! `Send + Sync`. bevy 0.19 still requires `Resource: Component:
//! Send + Sync`, so a plain `#[derive(Resource)]` does not work
//! even for a `NonSend`-style wrapper. Phase 7.5 keeps the
//! `Rc<RefCell<TrayHandle>>` parked on `AppState::tray` and
//! drains the `Messages<TickGtk>` queue from `runtime.rs`. The
//! resource module entry stays so the next phase can introduce
//! a `NonSend<...>` insertion path.
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
///
/// [`gtk::init`]: https://gtk-rs.org/gtk-rs-core/stable/latest/docs/gtk/fn.init.html
#[cfg(target_os = "linux")]
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct GtkReady(pub bool);
