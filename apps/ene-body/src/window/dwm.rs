//! Windows 11 layered/DWM transparent overlay.

#[cfg(target_os = "windows")]
mod imp {
    use std::collections::VecDeque;
    use std::num::NonZeroIsize;

    use raw_window_handle::{
        RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
    };
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, GetStockObject, HOLLOW_BRUSH, MONITOR_DEFAULTTONEAREST, MONITORINFO,
        MonitorFromWindow, ScreenToClient,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Controls::MARGINS;
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
        DispatchMessageW, GWLP_USERDATA, GetClientRect, GetForegroundWindow, GetWindowLongPtrW,
        GetWindowRect, HTBOTTOMRIGHT, HTCAPTION, HTTRANSPARENT, IDC_ARROW, LWA_ALPHA, LoadCursorW,
        MSG, PM_REMOVE, PeekMessageW, PostQuitMessage, RegisterClassExW, SW_HIDE,
        SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER, SetLayeredWindowAttributes,
        SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, WM_CLOSE, WM_DESTROY,
        WM_DPICHANGED, WM_ERASEBKGND, WM_MOVE, WM_NCCALCSIZE, WM_NCCREATE, WM_NCHITTEST, WM_SIZE,
        WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
        WS_THICKFRAME,
    };

    use crate::ipc::{
        GpuFailInfo, GpuFailReason, GpuInitStatus, LocalUiFact, PlacementBox, PresentationFeedback,
    };
    use crate::render::{RenderFailure, SurfaceRenderer};
    use crate::window::OverlayProbe;

    const CLASS_NAME: &[u16] = &[
        b'e' as u16,
        b'n' as u16,
        b'e' as u16,
        b'-' as u16,
        b'b' as u16,
        b'o' as u16,
        b'd' as u16,
        b'y' as u16,
        0,
    ];

    pub struct WindowsOverlay {
        hwnd: HWND,
        state: Box<WindowState>,
        renderer: Option<SurfaceRenderer>,
        visible: bool,
        gpu_failure: Option<GpuFailInfo>,
    }

    impl std::fmt::Debug for WindowsOverlay {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("WindowsOverlay")
                .field("visible", &self.visible)
                .field("placement", &self.state.placement)
                .field("gpu_ready", &self.renderer.is_some())
                .finish()
        }
    }

    impl WindowsOverlay {
        pub fn open(try_gpu: bool) -> Result<Self, String> {
            // SAFETY: process DPI awareness must be selected before creating
            // this process's first HWND. Failure can mean an embedding host
            // already selected an awareness context; GetDpiForWindow below is
            // authoritative for the created window.
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
            // SAFETY: null asks for the current process module.
            let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
            if instance.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            // SAFETY: all pointers in the class live for process lifetime and
            // window_proc has the required system ABI.
            let cursor = unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) };
            let class = WNDCLASSEXW {
                cbSize: u32::try_from(std::mem::size_of::<WNDCLASSEXW>())
                    .map_err(|_| String::from("WNDCLASSEXW size overflow"))?,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: std::ptr::null_mut(),
                hCursor: cursor,
                // SAFETY: HOLLOW_BRUSH is a process-global stock object.
                hbrBackground: unsafe { GetStockObject(HOLLOW_BRUSH) },
                lpszMenuName: std::ptr::null(),
                lpszClassName: CLASS_NAME.as_ptr(),
                hIconSm: std::ptr::null_mut(),
            };
            // A zero result can mean the class already exists in this process;
            // CreateWindowExW below remains the authoritative check.
            // SAFETY: class is fully initialized.
            let _atom = unsafe { RegisterClassExW(&class) };
            let placement = PlacementBox {
                x: 24,
                y: 24,
                width: 420,
                height: 640,
                scale: 1.0,
            };
            let mut state = Box::new(WindowState {
                placement,
                events: VecDeque::new(),
                hidden: true,
            });
            let state_ptr = std::ptr::from_mut::<WindowState>(&mut state);
            // SAFETY: class is registered, state_ptr remains stable in Box for
            // the HWND lifetime, and dimensions are bounded u32→i32 below.
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    CLASS_NAME.as_ptr(),
                    CLASS_NAME.as_ptr(),
                    WS_POPUP | WS_THICKFRAME,
                    placement.x,
                    placement.y,
                    i32::try_from(placement.width).unwrap_or(i32::MAX),
                    i32::try_from(placement.height).unwrap_or(i32::MAX),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    state_ptr.cast(),
                )
            };
            if hwnd.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            // SAFETY: hwnd is live and owned by this object.
            state.placement.scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
            let initial_width = physical(state.placement.width, state.placement.scale);
            let initial_height = physical(state.placement.height, state.placement.scale);
            // SAFETY: hwnd is live and the requested size is finite and bounded.
            unsafe {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    state.placement.x,
                    state.placement.y,
                    i32::try_from(initial_width).unwrap_or(i32::MAX),
                    i32::try_from(initial_height).unwrap_or(i32::MAX),
                    SWP_NOACTIVATE | SWP_NOZORDER,
                )
            };
            // SAFETY: hwnd is live; a -1 margin asks DWM for full client glass.
            let margins = MARGINS {
                cxLeftWidth: -1,
                cxRightWidth: -1,
                cyTopHeight: -1,
                cyBottomHeight: -1,
            };
            let _dwm = unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) };
            // SAFETY: hwnd is layered and alpha 255 preserves per-pixel swap
            // chain alpha while avoiding a global fade.
            let alpha_ok = unsafe { SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA) };
            if alpha_ok == 0 {
                // SAFETY: hwnd was created above and is owned here.
                unsafe { DestroyWindow(hwnd) };
                return Err(std::io::Error::last_os_error().to_string());
            }
            let mut gpu_failure = None;
            let renderer = if try_gpu {
                let hwnd_value =
                    NonZeroIsize::new(hwnd as isize).ok_or_else(|| String::from("null HWND"))?;
                let mut window_handle = Win32WindowHandle::new(hwnd_value);
                window_handle.hinstance = NonZeroIsize::new(instance as isize);
                // SAFETY: hwnd and module remain live and are used on the
                // creating thread until renderer is dropped.
                match unsafe {
                    pollster::block_on(SurfaceRenderer::new(
                        RawDisplayHandle::Windows(WindowsDisplayHandle::new()),
                        RawWindowHandle::Win32(window_handle),
                        initial_width,
                        initial_height,
                    ))
                } {
                    Ok(renderer) => Some(renderer),
                    Err(failure) => {
                        gpu_failure = Some(gpu_info(failure));
                        None
                    }
                }
            } else {
                gpu_failure = Some(GpuFailInfo {
                    reason: GpuFailReason::NoAdapter,
                });
                None
            };
            Ok(Self {
                hwnd,
                state,
                renderer,
                visible: false,
                gpu_failure,
            })
        }

        pub fn gpu_status(&self) -> GpuInitStatus {
            if self.renderer.is_some() {
                GpuInitStatus::Ok
            } else {
                GpuInitStatus::Failed
            }
        }

        pub fn gpu_failure(&self) -> Option<GpuFailInfo> {
            self.gpu_failure
        }

        pub fn set_visible(&mut self, visible: bool) {
            self.visible = visible;
            self.state.hidden = !visible;
            // SAFETY: hwnd is live and owned by this object.
            unsafe { ShowWindow(self.hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
        }

        pub fn visible(&self) -> bool {
            self.visible && !self.state.hidden
        }

        pub fn set_placement(&mut self, placement: PlacementBox) {
            self.state.placement = placement;
            // SAFETY: hwnd is live and owned by this object.
            self.state.placement.scale = unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.0;
            let physical_width = physical(placement.width, self.state.placement.scale);
            let physical_height = physical(placement.height, self.state.placement.scale);
            // SAFETY: hwnd is live; no z-order or activation change is needed.
            unsafe {
                SetWindowPos(
                    self.hwnd,
                    std::ptr::null_mut(),
                    placement.x,
                    placement.y,
                    i32::try_from(physical_width).unwrap_or(i32::MAX),
                    i32::try_from(physical_height).unwrap_or(i32::MAX),
                    SWP_NOACTIVATE | SWP_NOZORDER,
                )
            };
            if let Some(renderer) = &mut self.renderer {
                renderer.resize(physical_width, physical_height);
            }
        }

        pub fn placement(&self) -> PlacementBox {
            self.state.placement
        }

        pub fn take_local_ui(&mut self) -> Option<LocalUiFact> {
            self.state.events.pop_front()
        }

        pub fn take_presentation(&mut self) -> Option<PresentationFeedback> {
            None
        }

        pub fn pump(&mut self) {
            self.pump_messages();
        }

        pub fn ready_to_render(&self) -> bool {
            self.visible() && !foreground_is_fullscreen(self.hwnd) && self.renderer.is_some()
        }

        pub fn render(&mut self, meshes: &[crate::vrm::RenderMesh]) {
            if !self.ready_to_render() {
                return;
            }
            if let Some(renderer) = &mut self.renderer {
                if let Err(failure) = renderer.render(meshes) {
                    self.renderer = None;
                    self.gpu_failure = Some(gpu_info(failure));
                }
            }
        }

        fn pump_messages(&mut self) {
            let mut message = MSG::default();
            loop {
                // SAFETY: message is writable; filtering by hwnd keeps this
                // Body window's queue isolated.
                let found = unsafe { PeekMessageW(&mut message, self.hwnd, 0, 0, PM_REMOVE) };
                if found == 0 {
                    break;
                }
                // SAFETY: message came from PeekMessageW.
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            let mut client = RECT::default();
            // SAFETY: hwnd is live and client is writable.
            if unsafe { GetClientRect(self.hwnd, &mut client) } != 0 {
                let width = (client.right - client.left).max(1) as u32;
                let height = (client.bottom - client.top).max(1) as u32;
                let logical_width = logical(width, self.state.placement.scale);
                let logical_height = logical(height, self.state.placement.scale);
                if (logical_width, logical_height)
                    != (self.state.placement.width, self.state.placement.height)
                {
                    self.state.placement.width = logical_width;
                    self.state.placement.height = logical_height;
                    if let Some(renderer) = &mut self.renderer {
                        renderer.resize(width, height);
                    }
                }
            }
        }
    }

    impl Drop for WindowsOverlay {
        fn drop(&mut self) {
            self.renderer = None;
            // SAFETY: hwnd was created and is destroyed once here.
            unsafe { DestroyWindow(self.hwnd) };
        }
    }

    struct WindowState {
        placement: PlacementBox,
        events: VecDeque<LocalUiFact>,
        hidden: bool,
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_NCCREATE {
            let create = lparam as *const CREATESTRUCTW;
            if !create.is_null() {
                // SAFETY: WM_NCCREATE lparam points to CREATESTRUCTW and
                // lpCreateParams is the Box-stable WindowState pointer.
                let state = unsafe { (*create).lpCreateParams } as *mut WindowState;
                // SAFETY: hwnd is being initialized on its owner thread.
                unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize) };
            }
        }
        // SAFETY: value was installed during WM_NCCREATE and remains Box-live.
        let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
        match message {
            WM_NCHITTEST if !state.is_null() => {
                let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
                let y = ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32;
                let mut point = POINT { x, y };
                // SAFETY: hwnd is live and point is writable.
                unsafe { ScreenToClient(hwnd, &mut point) };
                // SAFETY: state pointer validity established above.
                let placement = unsafe { (*state).placement };
                let width = f64::from(physical(placement.width, placement.scale));
                let height = f64::from(physical(placement.height, placement.scale));
                let nx = (f64::from(point.x) - width * 0.5) / (width * 0.38);
                let ny = (f64::from(point.y) - height * 0.52) / (height * 0.48);
                if nx * nx + ny * ny > 1.0 {
                    return HTTRANSPARENT as LRESULT;
                }
                let resize_width = physical(placement.width, placement.scale);
                let resize_height = physical(placement.height, placement.scale);
                let grip = physical(32, placement.scale);
                if point.x >= i32::try_from(resize_width.saturating_sub(grip)).unwrap_or(i32::MAX)
                    && point.y
                        >= i32::try_from(resize_height.saturating_sub(grip)).unwrap_or(i32::MAX)
                {
                    return HTBOTTOMRIGHT as LRESULT;
                }
                return HTCAPTION as LRESULT;
            }
            WM_NCCALCSIZE if wparam != 0 => return 0,
            WM_MOVE if !state.is_null() => {
                let mut rect = RECT::default();
                // SAFETY: hwnd is live and rect writable.
                if unsafe { GetWindowRect(hwnd, &mut rect) } != 0 {
                    // SAFETY: state pointer validity established above.
                    unsafe {
                        (*state).placement.x = rect.left;
                        (*state).placement.y = rect.top;
                        (*state).events.push_back(LocalUiFact::Drag {
                            x: rect.left,
                            y: rect.top,
                        });
                    }
                }
                return 0;
            }
            WM_SIZE if !state.is_null() => {
                let physical_width = (lparam as u32 & 0xffff).max(1);
                let physical_height = ((lparam as u32 >> 16) & 0xffff).max(1);
                // SAFETY: state pointer validity established above.
                unsafe {
                    let width = logical(physical_width, (*state).placement.scale);
                    let height = logical(physical_height, (*state).placement.scale);
                    (*state).placement.width = width;
                    (*state).placement.height = height;
                    (*state)
                        .events
                        .push_back(LocalUiFact::Resize { width, height });
                }
                return 0;
            }
            WM_DPICHANGED if !state.is_null() => {
                let suggested = lparam as *const RECT;
                if !suggested.is_null() {
                    // SAFETY: WM_DPICHANGED lparam is a suggested RECT.
                    let rect = unsafe { *suggested };
                    let dpi = (wparam as u32 & 0xffff).max(96);
                    // SAFETY: hwnd is live and rectangle is compositor supplied.
                    unsafe {
                        (*state).placement.scale = dpi as f32 / 96.0;
                        SetWindowPos(
                            hwnd,
                            std::ptr::null_mut(),
                            rect.left,
                            rect.top,
                            rect.right - rect.left,
                            rect.bottom - rect.top,
                            SWP_NOACTIVATE | SWP_NOZORDER,
                        );
                    }
                }
                return 0;
            }
            WM_CLOSE if !state.is_null() => {
                // SAFETY: hwnd is live and state pointer validity established.
                unsafe {
                    ShowWindow(hwnd, SW_HIDE);
                    (*state).hidden = true;
                    (*state).events.push_back(LocalUiFact::Hide);
                }
                return 0;
            }
            WM_ERASEBKGND => return 1,
            WM_DESTROY => {
                // SAFETY: posts to this thread's message queue.
                unsafe { PostQuitMessage(0) };
                return 0;
            }
            _ => {}
        }
        // SAFETY: unhandled messages follow the Win32 default procedure.
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    fn physical(logical: u32, scale: f32) -> u32 {
        ((logical as f64 * f64::from(scale)).round() as u64).clamp(1, u64::from(u32::MAX)) as u32
    }

    fn logical(physical: u32, scale: f32) -> u32 {
        ((physical as f64 / f64::from(scale.max(f32::EPSILON))).round() as u64)
            .clamp(1, u64::from(u32::MAX)) as u32
    }

    fn gpu_info(failure: RenderFailure) -> GpuFailInfo {
        GpuFailInfo {
            reason: match failure {
                RenderFailure::Adapter => GpuFailReason::NoAdapter,
                RenderFailure::Device => GpuFailReason::RequestDevice,
                RenderFailure::Surface => GpuFailReason::Surface,
                RenderFailure::DeviceLost => GpuFailReason::DeviceLost,
                RenderFailure::OutOfMemory => GpuFailReason::OutOfMemory,
            },
        }
    }

    fn foreground_is_fullscreen(overlay: HWND) -> bool {
        // SAFETY: these are read-only window-manager queries. Every pointer
        // targets an initialized writable structure for the duration of call.
        unsafe {
            let foreground = GetForegroundWindow();
            if foreground.is_null() || foreground == overlay {
                return false;
            }
            let monitor = MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST);
            if monitor.is_null() {
                return false;
            }
            let mut monitor_info = MONITORINFO {
                cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(u32::MAX),
                rcMonitor: RECT::default(),
                rcWork: RECT::default(),
                dwFlags: 0,
            };
            let mut window = RECT::default();
            if GetMonitorInfoW(monitor, &mut monitor_info) == 0
                || GetWindowRect(foreground, &mut window) == 0
            {
                return false;
            }
            window.left <= monitor_info.rcMonitor.left
                && window.top <= monitor_info.rcMonitor.top
                && window.right >= monitor_info.rcMonitor.right
                && window.bottom >= monitor_info.rcMonitor.bottom
        }
    }

    pub fn probe() -> OverlayProbe {
        match WindowsOverlay::open(false) {
            Ok(_) => OverlayProbe::Available,
            Err(reason) => OverlayProbe::Unavailable { reason },
        }
    }
}

#[cfg(target_os = "windows")]
pub use imp::WindowsOverlay;

use super::OverlayProbe;

#[must_use]
pub fn windows_dwm_probe() -> OverlayProbe {
    #[cfg(target_os = "windows")]
    {
        imp::probe()
    }
    #[cfg(not(target_os = "windows"))]
    {
        OverlayProbe::Unavailable {
            reason: String::from("not Windows"),
        }
    }
}
