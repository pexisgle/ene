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
