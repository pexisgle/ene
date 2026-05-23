use bevy::prelude::*;
use bevy::window::{PrimaryWindow, RawHandleWrapper};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::ffi::{c_char, c_int, c_uchar, c_ulong, c_void};

use crate::platform::LinuxWindowBackend;

use super::super::WindowBackendState;

#[derive(Default)]
pub(super) struct LinuxTaskbarState {
    applied_window: Option<c_ulong>,
    warned_wayland: bool,
    warned_unsupported: bool,
}

pub(super) fn sync_primary_window_taskbar_visibility(
    raw_handles: Query<&RawHandleWrapper, With<PrimaryWindow>>,
    window_backend: Res<WindowBackendState>,
    mut state: NonSendMut<LinuxTaskbarState>,
) {
    if window_backend.backend == LinuxWindowBackend::Wayland {
        // Wayland has no cross-compositor equivalent for "skip taskbar" on app windows.
        if !state.warned_wayland {
            log::info!("wayland session detected: skip-taskbar hint is not universally supported");
            state.warned_wayland = true;
        }
        return;
    }

    let Ok(raw_handle) = raw_handles.single() else {
        return;
    };
    let Some((display, window)) = xlib_display_and_window(raw_handle) else {
        if !state.warned_unsupported {
            log::warn!("failed to apply X11 skip-taskbar hint: raw Xlib handles are unavailable");
            state.warned_unsupported = true;
        }
        return;
    };

    if state.applied_window == Some(window) {
        return;
    }

    match unsafe { apply_x11_skip_taskbar_hint(display, window) } {
        Ok(()) => state.applied_window = Some(window),
        Err(err) => log::warn!("failed to apply X11 skip-taskbar hint: {err}"),
    }
}

fn xlib_display_and_window(raw_handles: &RawHandleWrapper) -> Option<(*mut c_void, c_ulong)> {
    match (
        raw_handles.get_display_handle(),
        raw_handles.get_window_handle(),
    ) {
        (RawDisplayHandle::Xlib(display), RawWindowHandle::Xlib(window)) => {
            Some((display.display?.as_ptr(), window.window))
        }
        _ => None,
    }
}

unsafe fn apply_x11_skip_taskbar_hint(
    display: *mut c_void,
    window: c_ulong,
) -> Result<(), &'static str> {
    if display.is_null() || window == 0 {
        return Err("invalid X11 display/window");
    }

    let net_wm_state = unsafe { intern_atom(display, NET_WM_STATE)? };
    let skip_taskbar = unsafe { intern_atom(display, NET_WM_STATE_SKIP_TASKBAR)? };
    let skip_pager = unsafe { intern_atom(display, NET_WM_STATE_SKIP_PAGER)? };
    let atoms = [skip_taskbar, skip_pager];

    let status = unsafe {
        XChangeProperty(
            display,
            window,
            net_wm_state,
            XA_ATOM,
            32,
            PROP_MODE_APPEND,
            atoms.as_ptr().cast(),
            atoms.len() as c_int,
        )
    };
    if status == 0 {
        return Err("XChangeProperty returned failure");
    }

    let _ = unsafe { XFlush(display) };
    Ok(())
}

unsafe fn intern_atom(display: *mut c_void, name: &'static [u8]) -> Result<c_ulong, &'static str> {
    let atom = unsafe { XInternAtom(display, name.as_ptr().cast(), 0) };
    if atom == 0 {
        return Err("XInternAtom returned zero");
    }
    Ok(atom)
}

const XA_ATOM: c_ulong = 4;
const PROP_MODE_APPEND: c_int = 2;

const NET_WM_STATE: &[u8] = b"_NET_WM_STATE\0";
const NET_WM_STATE_SKIP_TASKBAR: &[u8] = b"_NET_WM_STATE_SKIP_TASKBAR\0";
const NET_WM_STATE_SKIP_PAGER: &[u8] = b"_NET_WM_STATE_SKIP_PAGER\0";

#[link(name = "X11")]
unsafe extern "C" {
    fn XInternAtom(
        display: *mut c_void,
        atom_name: *const c_char,
        only_if_exists: c_int,
    ) -> c_ulong;
    fn XChangeProperty(
        display: *mut c_void,
        window: c_ulong,
        property: c_ulong,
        property_type: c_ulong,
        format: c_int,
        mode: c_int,
        data: *const c_uchar,
        nelements: c_int,
    ) -> c_int;
    fn XFlush(display: *mut c_void) -> c_int;
}
