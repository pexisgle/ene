//! Wayland `zwlr_layer_shell_v1` detection and fallback policy.
//!
//! Winit 0.30's `Window::set_cursor_hittest` is a Windows-only
//! no-op. On Wayland the OS-level input region is the
//! `wl_surface::set_input_region` call. When the compositor
//! additionally advertises `zwlr_layer_shell_v1`, we can promote
//! the character window to a `Layer::Overlay` surface so per-pixel
//! alpha is composited above fullscreen windows and the
//! click-through story is consistent regardless of the
//! application behind the character.
//!
//! # Scope
//!
//! This module is the **detection** half of the layer-shell
//! integration. The runtime's `resumed` opens the same
//! stand-alone Wayland connection it uses for the input-region
//! context and asks the registry for `zwlr_layer_shell_v1`. The
//! status ([`LayerShellStatus`]) is cached on the [`AppState`]
//! so the click-through dispatcher can branch on it:
//!
//! - `LayerShellStatus::Available(_)` — the compositor is layer
//!   shell capable. The actual layer surface construction is
//!   deferred to a follow-up (the namespace, output binding, and
//!   `ack_configure` dance need their own design pass).
//! - `LayerShellStatus::Unavailable` — the compositor does not
//!   advertise layer shell. The runtime falls back to the
//!   xdg-shell + `F8` "freeze character window" hotkey policy.
//!
//! The stand-alone connection is **shared** with
//! [`super::wayland_region::WaylandInputRegionContext`] (the
//! runtime clones the `Connection` handle out of the region
//! context if one is present; otherwise a fresh connection is
//! opened).
//!
//! # Failure modes
//!
//! - `WAYLAND_DISPLAY` not set / socket missing — `try_new`
//!   returns `LayerShellStatus::Unavailable` and the runtime
//!   falls back to the `F8` freeze policy.
//! - Registry round-trip fails — same, `Unavailable`.
//! - `zwlr_layer_shell_v1` global absent — same, `Unavailable`.

#[cfg(target_os = "linux")]
use parking_lot::Mutex;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    protocol::wl_registry::{self, WlRegistry},
};
#[cfg(target_os = "linux")]
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum LayerShellStatus {
    /// The compositor does not advertise `zwlr_layer_shell_v1`
    /// (older Sway, GNOME on Wayland before 41, or a non-layer
    /// shell reference compositor). The runtime falls back to
    /// the xdg-shell + `F8` "freeze" hotkey policy.
    #[default]
    Unavailable,
    /// The compositor advertises `zwlr_layer_shell_v1`. The
    /// cached value is the **advertised protocol version** (1..=N)
    /// so the follow-up can pass it to `bind` without re-probing.
    Available(Arc<u32>),
}

#[derive(Debug, Default)]
pub struct LayerShellContext {
    cached: Option<LayerShellStatus>,
}

impl LayerShellContext {
    pub const fn new() -> Self {
        Self { cached: None }
    }

    /// `connection` is `None` when the runtime is not running
    /// under Wayland (X11, Windows, or the Wayland input-region
    /// context failed to construct). The probe is a no-op in
    /// that case and returns [`LayerShellStatus::Unavailable`].
    pub fn status(&mut self, connection: Option<&Connection>) -> LayerShellStatus {
        if let Some(cached) = self.cached.as_ref() {
            return cached.clone();
        }
        let result = match connection {
            Some(conn) => probe_layer_shell(conn),
            None => LayerShellStatus::Unavailable,
        };
        self.cached = Some(result.clone());
        result
    }

    /// Used when the runtime's connection has been replaced
    /// (e.g. after a `wl_display` disconnect). Not currently
    /// wired up — the connection is a long-lived handle for
    /// the lifetime of the process.
    #[cfg_attr(not(test), expect(dead_code, reason = "Test-only helper"))]
    pub fn invalidate(&mut self) {
        self.cached = None;
    }

    /// The runtime uses this for the first-dispatch log so
    /// the log carries whether the cache has been populated.
    pub const fn cached(&self) -> Option<&LayerShellStatus> {
        self.cached.as_ref()
    }
}

pub fn probe_layer_shell(connection: &Connection) -> LayerShellStatus {
    let Ok((globals, mut event_queue)) =
        wayland_client::globals::registry_queue_init::<LayerShellAppData>(connection)
    else {
        return LayerShellStatus::Unavailable;
    };
    let mut state = LayerShellAppData;

    if event_queue.roundtrip(&mut state).is_err() {
        return LayerShellStatus::Unavailable;
    }

    let target_name = zwlr_layer_shell_v1::ZwlrLayerShellV1::interface().name;
    let matched = globals.contents().with_list(|list| {
        list.iter()
            .find(|g| g.interface == target_name)
            .map(|g| g.version)
    });
    if let Some(version) = matched {
        LayerShellStatus::Available(Arc::new(version))
    } else {
        LayerShellStatus::Unavailable
    }
}

#[derive(Debug)]
struct LayerShellAppData;

impl Dispatch<WlRegistry, wayland_client::globals::GlobalListContents> for LayerShellAppData {
    fn event(
        _state: &mut Self,
        _registry: &WlRegistry,
        _event: wl_registry::Event,
        _udata: &wayland_client::globals::GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

pub type LayerShellState = Arc<Mutex<LayerShellContext>>;

pub fn new_layer_shell_state() -> LayerShellState {
    Arc::new(Mutex::new(LayerShellContext::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_shell_status_default_is_unavailable() {
        assert_eq!(LayerShellStatus::default(), LayerShellStatus::Unavailable);
    }

    #[test]
    fn layer_shell_context_starts_empty() {
        let ctx = LayerShellContext::new();
        assert!(ctx.cached.is_none());
    }

    #[test]
    fn layer_shell_context_invalidate_clears_cache() {
        let mut ctx = LayerShellContext::new();
        ctx.cached = Some(LayerShellStatus::Unavailable);
        ctx.invalidate();
        assert!(ctx.cached.is_none());
    }

    #[test]
    fn new_layer_shell_state_is_unique() {
        let a = new_layer_shell_state();
        let b = new_layer_shell_state();
        assert!(!Arc::ptr_eq(&a, &b));
    }
}
