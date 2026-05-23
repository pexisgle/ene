use bevy::prelude::*;
use bevy::window::RawHandleWrapper;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GWL_EXSTYLE, GetWindowLongPtrW, HTTRANSPARENT, SWP_FRAMECHANGED, SWP_NOMOVE,
            SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, WM_NCHITTEST,
            WS_EX_TRANSPARENT,
        },
    },
};

use crate::platform::windows_hwnd;

pub(super) fn register(app: &mut App) {
    app.init_non_send_resource::<WindowsHitTestState>();
}

pub(super) const WINDOWS_HIT_TEST_SUBCLASS_ID: usize = 0x5743_4854;

#[derive(Default)]
pub(super) struct WindowsHitTestState {
    hook: Option<WindowsHitTestHook>,
}

struct WindowsHitTestHook {
    hwnd: HWND,
    passthrough: Arc<AtomicBool>,
    ref_data: usize,
}

impl Drop for WindowsHitTestState {
    fn drop(&mut self) {
        if let Some(hook) = self.hook.take() {
            unsafe { uninstall_windows_hit_test_hook(hook) };
        }
    }
}

pub(super) fn sync_windows_hit_test_hook(
    raw_handle: Option<&RawHandleWrapper>,
    allows_input: bool,
    state: &mut WindowsHitTestState,
) -> bool {
    let hwnd = raw_handle.and_then(windows_hwnd);
    let Some(hwnd) = hwnd else {
        if let Some(hook) = state.hook.take() {
            unsafe { uninstall_windows_hit_test_hook(hook) };
        }
        return false;
    };

    let requires_reinstall = state.hook.as_ref().map_or(true, |hook| hook.hwnd != hwnd);
    if requires_reinstall {
        if let Some(hook) = state.hook.take() {
            unsafe { uninstall_windows_hit_test_hook(hook) };
        }

        match unsafe { install_windows_hit_test_hook(hwnd) } {
            Ok(hook) => state.hook = Some(hook),
            Err(err) => {
                log::warn!("failed to install windows hit-test subclass: {err}");
                return false;
            }
        }
    }

    if let Some(hook) = state.hook.as_ref() {
        let passthrough = !allows_input;
        hook.passthrough.store(passthrough, Ordering::Relaxed);
        unsafe { set_windows_mouse_passthrough(hook.hwnd, passthrough) };
        true
    } else {
        false
    }
}

unsafe fn install_windows_hit_test_hook(hwnd: HWND) -> Result<WindowsHitTestHook, &'static str> {
    let passthrough = Arc::new(AtomicBool::new(false));
    let ref_data = Box::into_raw(Box::new(passthrough.clone())) as usize;
    if unsafe {
        SetWindowSubclass(
            hwnd,
            Some(windows_hit_test_subclass_proc),
            WINDOWS_HIT_TEST_SUBCLASS_ID,
            ref_data,
        )
    } == 0
    {
        unsafe { drop(Box::from_raw(ref_data as *mut Arc<AtomicBool>)) };
        return Err("SetWindowSubclass returned failure");
    }

    Ok(WindowsHitTestHook {
        hwnd,
        passthrough,
        ref_data,
    })
}

unsafe fn uninstall_windows_hit_test_hook(hook: WindowsHitTestHook) {
    unsafe { set_windows_mouse_passthrough(hook.hwnd, false) };
    unsafe {
        RemoveWindowSubclass(
            hook.hwnd,
            Some(windows_hit_test_subclass_proc),
            WINDOWS_HIT_TEST_SUBCLASS_ID,
        );
    }
    if hook.ref_data != 0 {
        unsafe { drop(Box::from_raw(hook.ref_data as *mut Arc<AtomicBool>)) };
    }
}

unsafe fn set_windows_mouse_passthrough(hwnd: HWND, passthrough: bool) {
    let current_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let transparent_bit = WS_EX_TRANSPARENT as isize;
    let target_style = if passthrough {
        current_style | transparent_bit
    } else {
        current_style & !transparent_bit
    };
    if target_style == current_style {
        return;
    }
    unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, target_style) };
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
        )
    };
}

unsafe extern "system" fn windows_hit_test_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    ref_data: usize,
) -> LRESULT {
    if msg == WM_NCHITTEST && ref_data != 0 {
        let passthrough = unsafe { &*(ref_data as *const Arc<AtomicBool>) };
        if passthrough.load(Ordering::Relaxed) {
            return HTTRANSPARENT as LRESULT;
        }
    }

    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}
