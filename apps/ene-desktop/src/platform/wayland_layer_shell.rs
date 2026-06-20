//! PR5.4 / PR-LX.4: Wayland `zwlr_layer_shell_v1` detection and
//! fallback policy.
//!
//! Winit 0.30's `Window::set_cursor_hittest` is a Windows-only
//! no-op. On Wayland the OS-level input region is the
//! `wl_surface::set_input_region` call (PR5.3 / LX.2). When the
//! compositor additionally advertises `zwlr_layer_shell_v1`, we
//! can promote the character window to a `Layer::Overlay` surface
//! so per-pixel alpha is composited above fullscreen windows and
//! the click-through story is consistent regardless of the
//! application behind the character.
//!
//! # LX.4 scope
//!
//! This module is the **detection** half of PR5.4 / LX.4. The
//! runtime's `resumed` opens the same stand-alone Wayland
//! connection it uses for the input-region context and asks the
//! registry for `zwlr_layer_shell_v1`. The status
//! ([`LayerShellStatus`]) is cached on the [`AppState`] so the
//! click-through dispatcher can branch on it:
//!
//! - `LayerShellStatus::Available(_)` — the compositor is layer
//!   shell capable. The actual layer surface construction is
//!   deferred to a follow-up PR (the namespace, output binding,
//!   and `ack_configure` dance need their own design pass).
//! - `LayerShellStatus::Unavailable` — the compositor does not
//!   advertise layer shell. The runtime falls back to the
//!   xdg-shell + `F8` "freeze character window" hotkey policy
//!   described in §4 PR5.3 of the migration plan.
//!
//! # Architecture (LX.4)
//!
//! The stand-alone connection is **shared** with
//! [`super::wayland_region::WaylandInputRegionContext`] (the
//! runtime clones the `Connection` handle out of the region
//! context if one is present; otherwise a fresh connection is
//! opened). This avoids a second Wayland socket per process.
//!
//! # Failure modes
//!
//! - `WAYLAND_DISPLAY` not set / socket missing — the
//!   `try_new` call returns `LayerShellStatus::Unavailable` and
//!   the runtime falls back to the `F8` freeze policy.
//! - Registry round-trip fails — same, `Unavailable`.
//! - `zwlr_layer_shell_v1` global absent — same, `Unavailable`.
//! - The `ZwlrLayerShellV1` proxy is dropped on shutdown. It
//!   is **not** used to create layer surfaces in LX.4; the
//!   follow-up PR handles the actual `get_layer_surface` call.

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

/// Outcome of the layer-shell detection probe.
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
    /// so the follow-up PR can pass it to `bind` without
    /// re-probing.
    Available(Arc<u32>),
}

/// Shared state for the layer-shell detection probe. The probe
/// itself runs once (lazily) on the first call to
/// [`LayerShellContext::status`]; subsequent calls return the
/// cached [`LayerShellStatus`].
#[derive(Debug, Default)]
pub struct LayerShellContext {
    /// The cached detection result. `None` until the first call
    /// to [`Self::status`].
    cached: Option<LayerShellStatus>,
}

impl LayerShellContext {
    /// Create a fresh context with no detection result cached.
    pub fn new() -> Self {
        Self { cached: None }
    }

    /// Return the cached detection result, or run the probe now
    /// against the supplied connection.
    ///
    /// `connection` is `None` when the runtime is not running
    /// under Wayland (X11, Windows, or the Wayland input-region
    /// context failed to construct). The probe is a no-op in
    /// that case and returns [`LayerShellStatus::Unavailable`].
    ///
    /// The probe is cheap: a single `wl_display::get_registry`
    /// round-trip followed by a one-shot `dispatch_pending` to
    /// drain the `global` event. Subsequent calls return the
    /// cached result.
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

    /// Force a re-probe on the next [`Self::status`] call. The
    /// runtime uses this when the connection has been replaced
    /// (e.g. after a `wl_display` disconnect). Not currently
    /// wired up — the connection is a long-lived handle for
    /// the lifetime of the process.
    #[allow(dead_code)]
    pub fn invalidate(&mut self) {
        self.cached = None;
    }

    /// Borrow the cached detection result without triggering a
    /// probe. Returns `None` if the probe has not run yet.
    /// The runtime uses this for the first-dispatch log so
    /// the log carries whether the cache has been populated.
    pub fn cached(&self) -> Option<&LayerShellStatus> {
        self.cached.as_ref()
    }
}

/// Run the detection probe. Uses `wayland_client::globals::registry_queue_init`
/// to spin up a transient event queue + `WlRegistry`, drains
/// globals, and looks for `zwlr_layer_shell_v1` by interface
/// name. Returns [`LayerShellStatus::Available`] (with the
/// advertised version) if found, otherwise
/// [`LayerShellStatus::Unavailable`].
///
/// The actual `bind` of the `zwlr_layer_shell_v1` proxy is
/// deferred to a follow-up PR — the LX.4 minimum-scope is
/// "detect whether the compositor supports layer-shell so the
/// runtime can pick the right fallback policy", not "construct
/// a `Layer::Overlay` surface". Re-binding on the cached
/// version is a one-liner once the follow-up PR lands.
#[allow(dead_code)] // Called from `LayerShellContext::status` once that wiring lands.
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

/// Application data type for the transient event queue used by
/// the layer-shell detection probe. The `wl_registry::global`
/// event triggers the bind to `zwlr_layer_shell_v1` so the
/// cache survives subsequent dispatches.
#[allow(dead_code)] // type parameter for `registry_queue_init`; held by the call.
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

/// Shared, thread-safe wrapper around [`LayerShellContext`].
pub type LayerShellState = Arc<Mutex<LayerShellContext>>;

/// Build a fresh [`LayerShellState`].
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
        // We cannot probe without a real connection in unit tests
        // (the probe would try to talk to a Wayland socket that
        // does not exist in `cargo test`), but we can verify
        // that `invalidate` clears a manually-injected value.
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
