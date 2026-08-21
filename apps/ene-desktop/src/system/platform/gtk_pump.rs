//! Calls the platform-level `tick_gtk` helper on Linux so
//! the GTK main loop integration the tray uses continues to
//! process events. On non-Linux builds the function is a
//! no-op.
//!
//! Runs in `AppSet::Settings` (after the AI pump has had a
//! chance to fire) so the tray and the AI both get a chance
//! to enqueue per-frame work before the GTK pump clears the
//! queue.
#[cfg(target_os = "linux")]
use bevy_ecs::prelude::Res;
#[cfg(target_os = "linux")]
pub fn tick_gtk_system(ready: Option<Res<crate::resource::tray::GtkReady>>) {
    if !ready.is_some_and(|r| r.0) {
        return;
    }
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
}

#[cfg(not(target_os = "linux"))]
#[expect(
    dead_code,
    reason = "non-Linux stub kept for uniform system registration"
)]
pub fn tick_gtk_system() {}
