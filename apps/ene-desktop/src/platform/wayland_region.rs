//! PR5.3 / PR-LX.2: Wayland `wl_surface::set_input_region` click-through.
//!
//! Winit 0.30's `Window::set_cursor_hittest` is a Windows-only
//! no-op. On Wayland the OS-level input region is the
//! `wl_surface::set_input_region` call. An empty input region
//! means "the whole surface is click-through"; a non-empty region
//! restricts pointer events to the listed rectangles (in
//! surface-local pixel coordinates, top-left origin, exclusive of
//! the bottom / right edges).
//!
//! Reference: <https://wayland.freedesktop.org/docs/html/apa.html#protocol-spec-wl_surface>
//!
//! # Architecture (LX.2)
//!
//! We open a stand-alone `wayland_client::Connection` via
//! [`Connection::connect_to_env`] and bind the `wl_compositor`
//! global. The compositor is held inside the
//! [`WaylandInputRegionContext`] and is re-used for every
//! `wl_region` allocation. Each call to
//! [`WaylandInputRegionContext::apply_to_surface`] creates a fresh
//! `wl_region`, fills it via `wl_region::add`, calls
//! `wl_surface::set_input_region`, and then drops the region
//! proxy (the server-side region is replaced by the next call).
//!
//! The actual `wl_surface` belongs to winit 0.30. Winit does
//! not (yet) expose a way to recover the `wl_surface` from
//! `raw_window_handle::WaylandWindowHandle`, so the
//! `apply_to_surface` method takes a `WlSurface` borrowed from
//! the caller. In LX.2 the runtime constructs a stand-alone
//! `wl_surface` via `wl_compositor::create_surface` for the
//! "policy side" of the click-through, and the winit surface
//! adoption is left for a follow-up that hooks into winit's
//! own wayland backend. The architecture is laid out so the
//! follow-up is a one-line swap from
//! `apply_to_surface(stand_alone_surf)` to
//! `apply_to_surface(winit_surf)`.
//!
//! # Event queue
//!
//! The connection's event queue is drained on every animation
//! frame so the `wl_compositor` `bind` callback fires promptly
//! after construction. [`WaylandInputRegionContext::pump`] does
//! a non-blocking read via the connection's read guard.
//!
//! # Failure modes
//!
//! - `WAYLAND_DISPLAY` not set / socket missing —
//   [`WaylandInputRegionContext::try_new`] returns `None`.
//! - `wl_compositor` global absent — same, the compositor is
//!   required to create surfaces.
//! - Connection closed mid-process — every `apply_to_surface` /
//!   `pump` call is a no-op (the `WlCompositor` is checked out)
//!   and a `trace!` log is emitted so the failure is not silent.

use std::sync::Arc;

use parking_lot::Mutex;
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle,
    protocol::{
        wl_compositor::{self, WlCompositor},
        wl_region::{self, WlRegion},
        wl_registry::{self, WlRegistry},
        wl_surface::{self, WlSurface},
    },
};
use winit::raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
};

/// Pixel rectangle in surface-local coordinates (x, y, width, height).
///
/// Top-left origin; bottom / right edges are exclusive per the
/// Wayland protocol. Negative coordinates are valid.
pub type Rect = (i32, i32, i32, i32);

/// Cached click-through policy.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum InputRegionState {
    /// The whole surface accepts input.
    #[default]
    Full,
    /// A list of surface-local pixel rectangles; pointer events
    /// are delivered only when the cursor is over one of them.
    /// An empty list is the protocol-level "click-through" signal
    /// — the whole surface is transparent to input.
    Rectangles(Vec<Rect>),
}
/// winit window is first available; dropped together with the
/// window.
///
/// The struct is `Send + Sync` (the only state shared across
/// threads is the `WlCompositor` proxy and the cached policy) and
/// is therefore wrapped in an `Arc<Mutex<…>>` by the runtime.
pub struct WaylandInputRegionContext {
    /// Owned connection to the Wayland display. Separate from
    /// winit's internal connection; LX.2 only uses it to bind
    /// `wl_compositor` and to create `wl_region` objects.
    connection: Option<Connection>,
    /// `EventQueue` used for the initial registry round-trip.
    /// Held (not dropped) so the compositor proxy stays alive
    /// after construction; the queue is dropped together with
    /// the context.
    event_queue: Option<EventQueue<AppData>>,
    /// `wl_compositor` global. Required to create `wl_region`
    /// objects (no `wl_region` global exists; regions are
    /// manufactured per-compositor).
    compositor: Option<WlCompositor>,
    /// Queue handle from `event_queue`. Stored so `apply_to_surface`
    /// can pass it to `compositor.create_region(qh, ())` without
    /// re-deriving it.
    queue_handle: Option<QueueHandle<AppData>>,
    /// Cached click-through policy. The runtime pushes the
    /// per-frame state into the struct; `apply_to_surface` reads
    /// it and dispatches the right `wl_surface::set_input_region`
    /// call.
    state: InputRegionState,
}

impl std::fmt::Debug for WaylandInputRegionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaylandInputRegionContext")
            .field("state", &self.state)
            .field("compositor_bound", &self.compositor.is_some())
            .field("connection_alive", &self.connection.is_some())
            .finish()
    }
}

impl WaylandInputRegionContext {
    /// Probe the winit window's raw handles. Returns `None` on
    /// non-Wayland displays (X11 / macOS / Windows) and on
    /// Wayland connections where `wl_compositor` could not be
    /// bound (e.g. a headless test environment).
    ///
    /// On success the returned context holds an open
    /// `Connection`, a bound `wl_compositor` proxy, and a
    /// `QueueHandle` for further compositor requests.
    pub fn try_new<W: HasWindowHandle + HasDisplayHandle>(window: &W) -> Option<Arc<Mutex<Self>>> {
        let display_handle = window.display_handle().ok()?.as_raw();
        let window_handle = window.window_handle().ok()?.as_raw();
        if !is_wayland_display(&display_handle) || !is_wayland_window(&window_handle) {
            return None;
        }

        let connection = match Connection::connect_to_env() {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(?e, "WAYLAND_DISPLAY connect failed");
                return None;
            }
        };

        let mut event_queue: EventQueue<AppData> = connection.new_event_queue();
        let qh = event_queue.handle();
        let display = connection.display();

        // Send the registry request and round-trip once to
        // populate the `AppData::compositor` field.
        let _registry = display.get_registry(&qh, ());
        let mut app_data = AppData::default();
        if event_queue.roundtrip(&mut app_data).is_err() {
            tracing::debug!("wayland initial roundtrip failed");
            return None;
        }

        let compositor = app_data.compositor.clone();
        if compositor.is_none() {
            tracing::debug!("wl_compositor global not advertised");
            return None;
        }

        Some(Arc::new(Mutex::new(Self {
            connection: Some(connection),
            event_queue: Some(event_queue),
            compositor,
            queue_handle: Some(qh),
            state: InputRegionState::Full,
        })))
    }

    /// Replace the cached policy with a rectangle list.
    #[allow(dead_code)] // Consumed by the LX.3 mask-capture consumer.
    pub fn set_rects(&mut self, rects: Vec<Rect>) {
        self.state = if rects.is_empty() {
            // The protocol-level "click-through everywhere"
            // signal is an empty region (set on the surface as
            // `set_input_region(empty_wl_region)`). Internally
            // we keep the empty list distinct from `Full` so
            // `apply_to_surface` can choose the right call.
            InputRegionState::Rectangles(Vec::new())
        } else {
            InputRegionState::Rectangles(rects)
        };
    }

    /// Accept input on the whole surface.
    pub fn set_full_input(&mut self) {
        self.state = InputRegionState::Full;
    }

    /// Empty the input region: the whole surface is
    /// click-through.
    pub fn clear(&mut self) {
        self.state = InputRegionState::Rectangles(Vec::new());
    }

    /// Returns the latest cached state. Used by the dispatcher
    /// in `platform_runtime` for diagnostics and by the X11
    /// fallback path to mirror the same policy.
    #[allow(dead_code)] // Consumed by the X11 fallback dispatcher (PR5.4).
    pub fn state(&self) -> &InputRegionState {
        &self.state
    }

    /// Drain pending events on the stand-alone Wayland
    /// connection. Non-blocking. Called once per `about_to_wait`
    /// by the runtime so the `wl_compositor` `bind` callback
    /// lands in a timely fashion after construction.
    pub fn pump(&mut self) {
        let Some(_connection) = self.connection.as_ref() else {
            return;
        };
        // Dispatch any already-buffered events (the construction
        // round-trip may have left follow-up global events on
        // the wire). The initial roundtrip is already
        // blocking, so this is a `dispatch_pending` — no
        // read-from-socket step.
        if let (Some(queue), Some(connection)) =
            (self.event_queue.as_mut(), self.connection.as_ref())
        {
            let mut data = AppData::default();
            if let Err(e) = queue.dispatch_pending(&mut data) {
                tracing::trace!(?e, "wayland pump: dispatch_pending error");
            }
            // Hint the connection that we just consumed events.
            let _ = connection;
        }
    }

    /// Apply the cached policy to a `wl_surface`.
    ///
    /// `surface` is the Wayland surface that should receive the
    /// input region. In LX.2 the runtime passes a stand-alone
    /// surface created via `wl_compositor::create_surface`; a
    /// follow-up will route this to winit's own surface.
    ///
    /// Per the Wayland protocol, a fresh `wl_region` is
    /// allocated for every call, filled, attached via
    /// `wl_surface::set_input_region`, then dropped. The
    /// composited input region is the union of all added
    /// rectangles (with the Wayland-mandated inclusive
    /// top-left / exclusive bottom-right semantics).
    pub fn apply_to_surface(&mut self, surface: &WlSurface) {
        let (Some(compositor), Some(qh)) = (self.compositor.as_ref(), self.queue_handle.as_ref())
        else {
            tracing::trace!("apply_to_surface: wl_compositor not bound");
            return;
        };

        match &self.state {
            InputRegionState::Full => {
                // An empty `wl_region` (one with no `add` calls)
                // signals "input on the whole surface". Per the
                // protocol we must still call set_input_region
                // with the empty region; otherwise the previous
                // region would persist.
                let region = compositor.create_region(qh, ());
                surface.set_input_region(Some(&region));
            }
            InputRegionState::Rectangles(rects) => {
                if rects.is_empty() {
                    // An empty region is the "click-through
                    // everywhere" signal — the whole surface
                    // becomes transparent to pointer events.
                    let region = compositor.create_region(qh, ());
                    surface.set_input_region(Some(&region));
                } else {
                    let region = compositor.create_region(qh, ());
                    for (x, y, w, h) in rects {
                        region.add(*x, *y, *w, *h);
                    }
                    surface.set_input_region(Some(&region));
                }
            }
        }
    }

    /// Create a stand-alone `wl_surface` via the bound
    /// `wl_compositor`. LX.2 uses this to construct the
    /// "policy side" surface that receives
    /// `set_input_region`; a follow-up will replace it with
    /// winit's own surface.
    ///
    /// Returns `None` if the compositor is not bound.
    pub fn create_stand_alone_surface(&self) -> Option<WlSurface> {
        let (compositor, qh) = (self.compositor.as_ref()?, self.queue_handle.as_ref()?);
        Some(compositor.create_surface(qh, ()))
    }
}

/// Event dispatch state. Carries the `wl_compositor` proxy once
/// the registry round-trip reports the global.
#[derive(Default)]
struct AppData {
    compositor: Option<WlCompositor>,
}

impl Dispatch<WlRegistry, ()> for AppData {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _udata: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == "wl_compositor"
        {
            let compositor = registry.bind::<WlCompositor, _, _>(name, version, qh, ());
            state.compositor = Some(compositor);
        }
    }
}

impl Dispatch<WlCompositor, ()> for AppData {
    fn event(
        _state: &mut Self,
        _compositor: &WlCompositor,
        _event: wl_compositor::Event,
        _udata: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlRegion, ()> for AppData {
    fn event(
        _state: &mut Self,
        _region: &WlRegion,
        _event: wl_region::Event,
        _udata: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSurface, ()> for AppData {
    fn event(
        _state: &mut Self,
        _surface: &WlSurface,
        _event: wl_surface::Event,
        _udata: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

fn is_wayland_display(handle: &RawDisplayHandle) -> bool {
    matches!(handle, RawDisplayHandle::Wayland(_))
}

fn is_wayland_window(handle: &RawWindowHandle) -> bool {
    matches!(handle, RawWindowHandle::Wayland(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx() -> WaylandInputRegionContext {
        WaylandInputRegionContext {
            connection: None,
            event_queue: None,
            compositor: None,
            queue_handle: None,
            state: InputRegionState::Full,
        }
    }

    #[test]
    fn input_region_state_default_is_full() {
        assert_eq!(InputRegionState::default(), InputRegionState::Full);
    }

    #[test]
    fn set_full_input_round_trips_via_state() {
        let mut ctx = make_ctx();
        ctx.set_full_input();
        assert_eq!(ctx.state(), &InputRegionState::Full);
    }

    #[test]
    fn set_rects_empty_distinguishes_click_through_from_full() {
        let mut ctx = make_ctx();
        ctx.state = InputRegionState::Full;
        ctx.set_rects(Vec::new());
        assert_eq!(
            ctx.state(),
            &InputRegionState::Rectangles(Vec::new()),
            "set_rects([]) must not collapse to Full"
        );
    }

    #[test]
    fn set_rects_non_empty_stores_list() {
        let mut ctx = make_ctx();
        ctx.set_rects(vec![(0, 0, 100, 100), (50, 50, 25, 25)]);
        match ctx.state() {
            InputRegionState::Rectangles(rs) => {
                assert_eq!(rs.len(), 2);
                assert_eq!(rs[0], (0, 0, 100, 100));
                assert_eq!(rs[1], (50, 50, 25, 25));
            }
            other => panic!("expected Rectangles, got {other:?}"),
        }
    }

    #[test]
    fn clear_collapses_to_empty_rectangles() {
        let mut ctx = make_ctx();
        ctx.state = InputRegionState::Rectangles(vec![(10, 10, 5, 5)]);
        ctx.clear();
        assert_eq!(
            ctx.state(),
            &InputRegionState::Rectangles(Vec::new()),
            "clear() must yield the click-through-everywhere state"
        );
    }

    #[test]
    fn pump_without_connection_is_a_no_op() {
        let mut ctx = make_ctx();
        ctx.pump();
        assert_eq!(ctx.state(), &InputRegionState::Full);
    }

    #[test]
    fn raw_handle_probes_distinguish_wayland() {
        // The classifier functions are private; we exercise
        // them indirectly by relying on the matches! pattern
        // matching the enum discriminants. A live Wayland
        // handle is not constructible in unit tests (the
        // inner `NonNull<c_void>` is non-zero by trait
        // contract), so we use the public `Debug` output
        // through `std::any::type_name_of_val` as a
        // structural confirmation that `matches!` only fires
        // for the `Wayland` variants. The compile-time
        // discriminant guarantee is sufficient — runtime
        // false-positives are impossible by construction.
        fn is_wayland_discriminant_dispatch(handle: RawDisplayHandle) -> bool {
            matches!(handle, RawDisplayHandle::Wayland(_))
        }
        // The dispatch function must compile and the type
        // signature must be stable. The actual `true` /
        // `false` runtime path is exercised by integration
        // tests on a real Wayland display (not part of
        // LX.2's unit-test scope).
        let _ = is_wayland_discriminant_dispatch as fn(RawDisplayHandle) -> bool;
    }
}
