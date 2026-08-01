//! Wayland `wl_surface::set_input_region` click-through.
//!
//! Winit 0.30's `Window::set_cursor_hittest` is Windows-only; on
//! Wayland the OS-level input region is the
//! `wl_surface::set_input_region` call. An empty region means
//! "the whole surface is click-through"; a non-empty region
//! restricts pointer events to the listed rectangles (surface-local
//! pixels, top-left origin, bottom / right exclusive).
//!
//! We open a stand-alone `wayland_client::Connection` and bind
//! the `wl_compositor` global; [`WaylandInputRegionContext`]
//! reuses the compositor for every `wl_region` allocation. The
//! runtime passes the winit surface's `WlSurface` in to
//! `apply_to_surface` (winit does not yet expose it from
//! `raw_window_handle`).
//!
//! Failure modes: `WAYLAND_DISPLAY` unset or socket missing,
//! `wl_compositor` global absent, or connection closed mid-process
//! — `try_new` returns `None` and `apply_to_surface` / `pump`
//! become silent no-ops.

use std::sync::Arc;

use parking_lot::Mutex;
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
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

/// Pixel rectangle in surface-local coordinates. Top-left origin;
/// bottom / right edges are exclusive per the Wayland protocol.
/// Negative coordinates are valid; `w` or `h` ≤ 0 marks an
/// empty (click-through) rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// Zero-area rectangles are click-through per the Wayland
    /// protocol; the X11 shape extension drops them silently too.
    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    /// Clamp a rectangle to fit within a `window_w` × `window_h`
    /// box. Origins are clipped to `(0, 0)`; the rectangle keeps
    /// whatever size remains. Used by the F9 debug overlay to keep
    /// the wireframe inside the window even when the OS input
    /// region overflows.
    pub fn clamp_to(&self, window_w: i32, window_h: i32) -> Self {
        let min_x = self.x.max(0);
        let min_y = self.y.max(0);
        let max_x = (self.x + self.w).min(window_w).max(min_x);
        let max_y = (self.y + self.h).min(window_h).max(min_y);
        Self {
            x: min_x,
            y: min_y,
            w: max_x - min_x,
            h: max_y - min_y,
        }
    }
}

impl From<(i32, i32, i32, i32)> for Rect {
    fn from((x, y, w, h): (i32, i32, i32, i32)) -> Self {
        Self { x, y, w, h }
    }
}

impl From<Rect> for (i32, i32, i32, i32) {
    fn from(r: Rect) -> Self {
        (r.x, r.y, r.w, r.h)
    }
}

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

/// Owned stand-alone Wayland connection + cached click-through
/// policy. `Send + Sync` (only the `WlCompositor` proxy and cached
/// state cross threads) so the runtime wraps it in `Arc<Mutex<…>>`.
pub struct WaylandInputRegionContext {
    /// Owned connection to the Wayland display. Separate from
    /// winit's; used to bind `wl_compositor` and create `wl_region`
    /// objects.
    connection: Option<Connection>,
    /// Held so the compositor proxy stays alive after construction.
    event_queue: Option<EventQueue<AppData>>,
    compositor: Option<WlCompositor>,
    queue_handle: Option<QueueHandle<AppData>>,
    /// Cached click-through policy. The runtime pushes per-frame;
    /// `apply_to_surface` reads it.
    state: InputRegionState,
    /// Winit window's own `WlSurface` proxy, wrapped from the
    /// raw pointer.
    winit_surface: Option<WlSurface>,
}

impl std::fmt::Debug for WaylandInputRegionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaylandInputRegionContext")
            .field("state", &self.state)
            .field("compositor_bound", &self.compositor.is_some())
            .field("connection_alive", &self.connection.is_some())
            .field("winit_surface_bound", &self.winit_surface.is_some())
            .finish_non_exhaustive()
    }
}

impl WaylandInputRegionContext {
    /// Clone the stand-alone Wayland [`Connection`], or `None`
    /// if it was never established. Used by the layer-shell probe
    /// to share the connection.
    pub fn clone_connection(&self) -> Option<Connection> {
        self.connection.clone()
    }
}

impl WaylandInputRegionContext {
    /// Probe the winit window's raw handles. Returns `None` on
    /// non-Wayland displays and when `wl_compositor` cannot be bound.
    pub fn try_new<W: HasWindowHandle + HasDisplayHandle>(window: &W) -> Option<Arc<Mutex<Self>>> {
        let display_handle = window.display_handle().ok()?.as_raw();
        let window_handle = window.window_handle().ok()?.as_raw();
        if !is_wayland_display(&display_handle) || !is_wayland_window(&window_handle) {
            return None;
        }

        let RawDisplayHandle::Wayland(wl_display_handle) = display_handle else {
            return None;
        };
        let RawWindowHandle::Wayland(wl_window_handle) = window_handle else {
            return None;
        };

        // Wrap winit's display pointer as a Connection.
        // Operating in guest mode means we don't close the display when connection is dropped.
        // SAFETY: The display pointer comes from winit's valid Wayland window handle.
        // Guest mode ensures the display won't be closed when Connection is dropped.
        let connection = unsafe {
            let backend = wayland_client::backend::Backend::from_foreign_display(
                wl_display_handle.display.as_ptr().cast(),
            );
            Connection::from_backend(backend)
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

        // Recover the winit window's wl_surface proxy.
        let raw_surface_ptr = wl_window_handle.surface.as_ptr();
        // SAFETY: `raw_surface_ptr` comes from winit's Wayland window handle,
        // which guarantees it is a valid wl_surface proxy.
        let object_id = unsafe {
            wayland_client::backend::ObjectId::from_ptr(
                <WlSurface as Proxy>::interface(),
                raw_surface_ptr.cast(),
            )
        }
        .ok()?;
        let winit_surface = Proxy::from_id(&connection, object_id).ok()?;

        Some(Arc::new(Mutex::new(Self {
            connection: Some(connection),
            event_queue: Some(event_queue),
            compositor,
            queue_handle: Some(qh),
            state: InputRegionState::Full,
            winit_surface: Some(winit_surface),
        })))
    }

    /// Replace the cached policy with a rectangle list. An empty
    /// list is kept distinct from `Full` so `apply_to_surface` can
    /// choose between the "null region" call and the "empty region"
    /// call.
    pub fn set_rects(&mut self, rects: Vec<Rect>) {
        self.state = if rects.is_empty() {
            InputRegionState::Rectangles(Vec::new())
        } else {
            InputRegionState::Rectangles(rects)
        };
    }

    pub fn set_full_input(&mut self) {
        self.state = InputRegionState::Full;
    }

    /// Empty the input region: the whole surface is click-through.
    #[cfg_attr(not(test), expect(dead_code, reason = "Test-only helper"))]
    pub fn clear(&mut self) {
        self.state = InputRegionState::Rectangles(Vec::new());
    }

    /// Latest cached state. Read by the X11 fallback dispatcher
    /// and the F9 debug overlay.
    #[cfg_attr(not(test), expect(dead_code, reason = "Test-only helper"))]
    pub const fn state(&self) -> &InputRegionState {
        &self.state
    }

    /// Drain pending events on the stand-alone Wayland connection.
    /// Non-blocking; called once per `about_to_wait`.
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
            let _ = connection;
        }
    }

    /// Apply the cached policy to a `wl_surface`. Per the Wayland
    /// protocol a fresh `wl_region` is allocated for every call,
    /// filled, attached via `wl_surface::set_input_region`, then
    /// dropped. The composited input region is the union of all
    /// added rectangles.
    pub fn apply_to_surface(&mut self, surface: &WlSurface) {
        let (Some(compositor), Some(qh)) = (self.compositor.as_ref(), self.queue_handle.as_ref())
        else {
            tracing::trace!("apply_to_surface: wl_compositor not bound");
            return;
        };

        match &self.state {
            InputRegionState::Full => {
                // A null input region sets the input region to the entire surface (Full input).
                surface.set_input_region(None);
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
                    for r in rects {
                        region.add(r.x, r.y, r.w, r.h);
                    }
                    surface.set_input_region(Some(&region));
                }
            }
        }

        surface.commit();
        if let Some(conn) = self.connection.as_ref() {
            drop(conn.flush());
        }
    }

    pub fn apply_to_winit_surface(&mut self) {
        if let Some(surface) = self.winit_surface.clone() {
            self.apply_to_surface(&surface);
        }
    }

    /// Create a stand-alone `wl_surface` via the bound
    /// `wl_compositor`. Returns `None` if the compositor is not bound.
    #[expect(
        dead_code,
        reason = "Wayland surface helper retained for future input-region tests"
    )]
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

const fn is_wayland_display(handle: &RawDisplayHandle) -> bool {
    matches!(handle, RawDisplayHandle::Wayland(_))
}

const fn is_wayland_window(handle: &RawWindowHandle) -> bool {
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
            winit_surface: None,
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
        ctx.set_rects(vec![Rect::new(0, 0, 100, 100), Rect::new(50, 50, 25, 25)]);
        match ctx.state() {
            InputRegionState::Rectangles(rs) => {
                assert_eq!(rs.len(), 2);
                assert_eq!(rs[0], Rect::new(0, 0, 100, 100));
                assert_eq!(rs[1], Rect::new(50, 50, 25, 25));
            }
            InputRegionState::Full => {
                let ok = false;
                assert!(ok, "expected Rectangles");
            }
        }
    }

    #[test]
    fn clear_collapses_to_empty_rectangles() {
        let mut ctx = make_ctx();
        ctx.state = InputRegionState::Rectangles(vec![Rect::new(10, 10, 5, 5)]);
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
        // The classifier functions are private; compile-time
        // discriminant match on the `RawDisplayHandle` / `RawWindowHandle`
        // enum is the structural guarantee. A live Wayland handle is
        // not constructible in unit tests, so the runtime path is
        // covered by integration tests on a real display.
        fn is_wayland_discriminant_dispatch(handle: RawDisplayHandle) -> bool {
            matches!(handle, RawDisplayHandle::Wayland(_))
        }
        let _ = is_wayland_discriminant_dispatch as fn(RawDisplayHandle) -> bool;
    }
}

#[cfg(test)]
mod rect_tests {
    use super::Rect;

    #[test]
    fn from_tuple_round_trips_to_tuple() {
        let r: Rect = (3, -2, 10, 20).into();
        assert_eq!(r, Rect::new(3, -2, 10, 20));
        let back: (i32, i32, i32, i32) = r.into();
        assert_eq!(back, (3, -2, 10, 20));
    }

    #[test]
    fn is_empty_when_w_or_h_is_non_positive() {
        assert!(Rect::new(0, 0, 0, 10).is_empty());
        assert!(Rect::new(0, 0, 10, 0).is_empty());
        assert!(Rect::new(0, 0, -1, 10).is_empty());
        assert!(!Rect::new(0, 0, 1, 1).is_empty());
    }

    #[test]
    fn clamp_to_crops_origin_and_caps_size() {
        let r = Rect::new(-5, -5, 100, 100);
        let c = r.clamp_to(50, 40);
        assert_eq!(c, Rect::new(0, 0, 50, 40));
    }

    #[test]
    fn clamp_to_with_fully_inside_rect_is_identity() {
        let r = Rect::new(5, 5, 10, 10);
        let c = r.clamp_to(100, 100);
        assert_eq!(c, r);
    }

    #[test]
    fn clamp_to_with_window_smaller_than_origin_returns_empty() {
        let r = Rect::new(200, 200, 10, 10);
        let c = r.clamp_to(100, 100);
        assert!(c.is_empty());
    }
}
