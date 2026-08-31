//! Wayland `wl_surface::set_input_region` from [`InteractionGeometry`].

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
use winit::window::Window;

use crate::interaction_controller::InteractionMode;
use crate::scene::InteractionGeometry;

use super::rects_i32;

pub struct WaylandRegion {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    connection: Connection,
    event_queue: EventQueue<AppData>,
    compositor: WlCompositor,
    queue_handle: QueueHandle<AppData>,
    surface: WlSurface,
    native: bool,
}

impl WaylandRegion {
    pub fn try_new(window: &Window) -> Option<Self> {
        let display_handle = window.display_handle().ok()?.as_raw();
        let window_handle = window.window_handle().ok()?.as_raw();
        let RawDisplayHandle::Wayland(wl_display) = display_handle else {
            return None;
        };
        let RawWindowHandle::Wayland(wl_window) = window_handle else {
            return None;
        };
        // SAFETY: winit's Wayland display pointer is valid for the window lifetime.
        // Guest mode does not close the display when this Connection is dropped.
        let connection = unsafe {
            let backend = wayland_client::backend::Backend::from_foreign_display(
                wl_display.display.as_ptr().cast(),
            );
            Connection::from_backend(backend)
        };
        let mut event_queue: EventQueue<AppData> = connection.new_event_queue();
        let qh = event_queue.handle();
        let display = connection.display();
        let _registry = display.get_registry(&qh, ());
        let mut app_data = AppData::default();
        if event_queue.roundtrip(&mut app_data).is_err() {
            tracing::debug!("wayland initial roundtrip failed");
            return None;
        }
        let compositor = app_data.compositor?;
        let raw_surface_ptr = wl_window.surface.as_ptr();
        // SAFETY: pointer comes from winit's live WlSurface.
        let object_id = unsafe {
            wayland_client::backend::ObjectId::from_ptr(
                <WlSurface as Proxy>::interface(),
                raw_surface_ptr.cast(),
            )
        }
        .ok()?;
        let surface = Proxy::from_id(&connection, object_id).ok()?;
        tracing::info!(
            native = true,
            "Wayland overlay input regions enabled (Weston 13 is the guaranteed compositor)"
        );
        Some(Self {
            inner: Arc::new(Mutex::new(Inner {
                connection,
                event_queue,
                compositor,
                queue_handle: qh,
                surface,
                native: true,
            })),
        })
    }

    pub fn apply(&mut self, mode: InteractionMode, interaction: &InteractionGeometry) {
        let mut inner = self.inner.lock();
        let mut data = AppData::default();
        if let Err(err) = inner.event_queue.dispatch_pending(&mut data) {
            tracing::trace!(error = %err, "wayland dispatch_pending");
        }
        let qh = inner.queue_handle.clone();
        match mode {
            InteractionMode::Passive => {
                let region = inner.compositor.create_region(&qh, ());
                inner.surface.set_input_region(Some(&region));
            }
            InteractionMode::Dragging | InteractionMode::UiFocused => {
                inner.surface.set_input_region(None);
            }
            InteractionMode::Interactive => {
                let rects = rects_i32(&interaction.rects);
                let region = inner.compositor.create_region(&qh, ());
                for (x, y, w, h) in rects {
                    region.add(x, y, w, h);
                }
                inner.surface.set_input_region(Some(&region));
            }
        }
        inner.surface.commit();
        drop(inner.connection.flush());
        let _ = inner.native;
    }
}

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
            state.compositor = Some(registry.bind::<WlCompositor, _, _>(name, version, qh, ()));
        }
    }
}

impl Dispatch<WlCompositor, ()> for AppData {
    fn event(
        _state: &mut Self,
        _proxy: &WlCompositor,
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
        _proxy: &WlRegion,
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
        _proxy: &WlSurface,
        _event: wl_surface::Event,
        _udata: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}
