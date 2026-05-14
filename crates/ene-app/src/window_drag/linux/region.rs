use bevy::prelude::*;
use bevy::window::{PrimaryWindow, RawHandleWrapper};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::ffi::c_void;
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    backend::ObjectId,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_compositor, wl_region, wl_registry, wl_surface},
};

use super::super::WindowBackendState;
use super::capture::{MaskRect, WaylandMaskCaptureState, WaylandVisibleRectState};
use crate::platform::LinuxWindowBackend;

#[derive(Default)]
pub(super) struct WaylandInputRegionState {
    context: Option<WaylandInputRegionContext>,
    last_rects: Vec<MaskRect>,
    last_source_size: UVec2,
    last_window_size: UVec2,
}

struct WaylandInputRegionContext {
    display_ptr: *mut c_void,
    surface_ptr: *mut c_void,
    connection: Connection,
    event_queue: EventQueue<WaylandRegionDispatchState>,
    dispatch_state: WaylandRegionDispatchState,
    queue_handle: QueueHandle<WaylandRegionDispatchState>,
    compositor: wl_compositor::WlCompositor,
    surface: wl_surface::WlSurface,
}

#[derive(Default)]
struct WaylandRegionDispatchState;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for WaylandRegionDispatchState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for WaylandRegionDispatchState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_compositor::WlCompositor,
        _event: wl_compositor::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_region::WlRegion, ()> for WaylandRegionDispatchState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_region::WlRegion,
        _event: wl_region::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for WaylandRegionDispatchState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_surface::WlSurface,
        _event: wl_surface::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

pub(super) fn apply_wayland_input_region(
    window_and_handles: Query<(&Window, &RawHandleWrapper), With<PrimaryWindow>>,
    rect_state: Res<WaylandVisibleRectState>,
    capture_state: Res<WaylandMaskCaptureState>,
    window_backend: Res<WindowBackendState>,
    mut region_state: NonSendMut<WaylandInputRegionState>,
) {
    if window_backend.backend != LinuxWindowBackend::Wayland {
        region_state.context = None;
        reset_wayland_input_region_cache(&mut region_state);
        return;
    }

    let Ok((window, raw_handles)) = window_and_handles.single() else {
        return;
    };
    let Some((display_ptr, surface_ptr)) = wayland_raw_pointers(raw_handles) else {
        region_state.context = None;
        reset_wayland_input_region_cache(&mut region_state);
        return;
    };

    let requires_reinitialize = region_state.context.as_ref().is_none_or(|context| {
        context.display_ptr != display_ptr || context.surface_ptr != surface_ptr
    });
    if requires_reinitialize {
        match initialize_wayland_region_context(display_ptr, surface_ptr) {
            Ok(context) => {
                region_state.context = Some(context);
                reset_wayland_input_region_cache(&mut region_state);
            }
            Err(err) => {
                log::warn!("failed to initialize wayland input-region bridge: {err}");
                region_state.context = None;
                return;
            }
        }
    }

    let Some(source_size) = (if rect_state.source_size.x > 0 && rect_state.source_size.y > 0 {
        Some(rect_state.source_size)
    } else if capture_state.texture_size.x > 0 && capture_state.texture_size.y > 0 {
        Some(capture_state.texture_size)
    } else {
        None
    }) else {
        return;
    };
    let window_size = window.physical_size();
    if region_state.last_rects == rect_state.rects
        && region_state.last_source_size == source_size
        && region_state.last_window_size == window_size
    {
        return;
    }

    let Some(context) = region_state.context.as_mut() else {
        return;
    };
    if let Err(err) =
        apply_wayland_region_to_surface(context, &rect_state.rects, source_size, window_size)
    {
        log::warn!("failed to apply wayland input region: {err}");
        region_state.context = None;
        reset_wayland_input_region_cache(&mut region_state);
        return;
    }

    region_state.last_rects.clear();
    region_state.last_rects.extend_from_slice(&rect_state.rects);
    region_state.last_source_size = source_size;
    region_state.last_window_size = window_size;
}

fn reset_wayland_input_region_cache(region_state: &mut WaylandInputRegionState) {
    region_state.last_rects.clear();
    region_state.last_source_size = UVec2::ZERO;
    region_state.last_window_size = UVec2::ZERO;
}

fn wayland_raw_pointers(handles: &RawHandleWrapper) -> Option<(*mut c_void, *mut c_void)> {
    match (handles.get_display_handle(), handles.get_window_handle()) {
        (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(window)) => {
            Some((display.display.as_ptr(), window.surface.as_ptr()))
        }
        _ => None,
    }
}

fn initialize_wayland_region_context(
    display_ptr: *mut c_void,
    surface_ptr: *mut c_void,
) -> Result<WaylandInputRegionContext, String> {
    // SAFETY: We reuse pointers provided by the active Bevy/winit Wayland window.
    let backend =
        unsafe { wayland_client::backend::Backend::from_foreign_display(display_ptr.cast()) };
    let connection = Connection::from_backend(backend);
    let (globals, mut event_queue) = registry_queue_init::<WaylandRegionDispatchState>(&connection)
        .map_err(|err| format!("registry init failed: {err}"))?;
    let queue_handle = event_queue.handle();
    let compositor = globals
        .bind::<wl_compositor::WlCompositor, _, _>(&queue_handle, 1..=6, ())
        .map_err(|err| format!("wl_compositor bind failed: {err}"))?;

    // SAFETY: The surface pointer originates from RawWindowHandle and remains valid for this window.
    let surface_id =
        unsafe { ObjectId::from_ptr(wl_surface::WlSurface::interface(), surface_ptr.cast()) }
            .map_err(|err| format!("wl_surface id conversion failed: {err}"))?;
    let surface = wl_surface::WlSurface::from_id(&connection, surface_id)
        .map_err(|err| format!("wl_surface proxy creation failed: {err}"))?;

    let mut dispatch_state = WaylandRegionDispatchState;
    event_queue
        .roundtrip(&mut dispatch_state)
        .map_err(|err| format!("initial wayland roundtrip failed: {err}"))?;

    Ok(WaylandInputRegionContext {
        display_ptr,
        surface_ptr,
        connection,
        event_queue,
        dispatch_state,
        queue_handle,
        compositor,
        surface,
    })
}

fn apply_wayland_region_to_surface(
    context: &mut WaylandInputRegionContext,
    rects: &[MaskRect],
    source_size: UVec2,
    window_size: UVec2,
) -> Result<(), String> {
    let region = context.compositor.create_region(&context.queue_handle, ());
    for rect in rects {
        if let Some((x, y, width, height)) =
            mask_rect_to_surface_rect(*rect, source_size, window_size)
        {
            region.add(x, y, width, height);
        }
    }
    context.surface.set_input_region(Some(&region));
    context.surface.commit();
    context
        .connection
        .flush()
        .map_err(|err| format!("wayland flush failed: {err}"))?;
    context
        .event_queue
        .dispatch_pending(&mut context.dispatch_state)
        .map_err(|err| format!("wayland dispatch failed: {err}"))?;
    Ok(())
}

fn mask_rect_to_surface_rect(
    rect: MaskRect,
    source_size: UVec2,
    window_size: UVec2,
) -> Option<(i32, i32, i32, i32)> {
    if source_size.x == 0 || source_size.y == 0 || window_size.x == 0 || window_size.y == 0 {
        return None;
    }

    let scale_x = window_size.x as f32 / source_size.x as f32;
    let scale_y = window_size.y as f32 / source_size.y as f32;
    let max_x = i32::try_from(window_size.x).unwrap_or(i32::MAX);
    let max_y = i32::try_from(window_size.y).unwrap_or(i32::MAX);

    let x0 = ((rect.x0 as f32) * scale_x).floor() as i32;
    let y0 = ((rect.y0 as f32) * scale_y).floor() as i32;
    let x1 = ((rect.x1 as f32) * scale_x).ceil() as i32;
    let y1 = ((rect.y1 as f32) * scale_y).ceil() as i32;

    let x0 = x0.clamp(0, max_x);
    let y0 = y0.clamp(0, max_y);
    let x1 = x1.clamp(0, max_x);
    let y1 = y1.clamp(0, max_y);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0, y0, x1 - x0, y1 - y0))
}
