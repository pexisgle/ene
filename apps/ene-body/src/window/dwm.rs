//! Windows 11 layered/DWM transparent overlay.

#[cfg(target_os = "windows")]
mod imp {
    use std::collections::VecDeque;
    use std::num::NonZeroIsize;

    use raw_window_handle::{
        RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
    };
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateRectRgn, DeleteObject, ExtCreateRegion, GetMonitorInfoW, GetStockObject,
        HOLLOW_BRUSH, HRGN, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        RDH_RECTANGLES, RGNDATA, RGNDATAHEADER, ScreenToClient, SetWindowRgn,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
        DispatchMessageW, GWLP_USERDATA, GetClientRect, GetForegroundWindow, GetWindowLongPtrW,
        GetWindowRect, HTBOTTOMRIGHT, HTCAPTION, HTTRANSPARENT, IDC_ARROW, LoadCursorW, MSG,
        PM_REMOVE, PeekMessageW, PostQuitMessage, RegisterClassExW, SW_HIDE, SW_SHOWNOACTIVATE,
        SWP_NOACTIVATE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, ShowWindow,
        TranslateMessage, WM_CLOSE, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_MOVE,
        WM_NCCALCSIZE, WM_NCCREATE, WM_NCHITTEST, WM_SIZE, WNDCLASSEXW, WS_EX_NOACTIVATE,
        WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_THICKFRAME,
    };

    use crate::ipc::{
        GpuFailInfo, GpuFailReason, GpuInitStatus, LocalUiFact, PlacementBox, PresentationFeedback,
    };
    use crate::render::{HitTestMask, RenderFailure, RenderOutcome, SurfaceRenderer};
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
                hit_test_mask: HitTestMask::empty(placement.width, placement.height),
                region_dirty: true,
                defer_region_refresh_once: false,
                region_failed: false,
                dpi_resize_in_progress: false,
            });
            let state_ptr = std::ptr::from_mut::<WindowState>(&mut state);
            // SAFETY: class is registered, state_ptr remains stable in Box for
            // the HWND lifetime, and dimensions are bounded u32→i32 below.
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
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
            let initial_mask = HitTestMask::empty(initial_width, initial_height);
            if let Err(reason) = apply_input_region(hwnd, &initial_mask) {
                // SAFETY: hwnd was successfully created and has not yet been
                // transferred into WindowsOverlay.
                unsafe { DestroyWindow(hwnd) };
                return Err(reason);
            }
            state.hit_test_mask = initial_mask;
            state.region_dirty = true;
            state.region_failed = false;
            // DirectComposition supplies the per-pixel alpha. A layered/GDI
            // redirection bitmap would put an opaque surface behind the visual.
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
            let show = visible && self.renderer.is_some() && !self.state.region_failed;
            self.state.hidden = !show;
            // SAFETY: hwnd is live and owned by this object.
            unsafe { ShowWindow(self.hwnd, if show { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
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
            self.state.region_dirty = true;
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
            if self.state.region_failed {
                self.fail_surface();
            }
        }

        pub fn ready_to_render(&self) -> bool {
            self.visible() && !foreground_is_fullscreen(self.hwnd) && self.renderer.is_some()
        }

        pub fn render(&mut self, meshes: &[crate::vrm::RenderMesh]) {
            if !self.ready_to_render() {
                return;
            }
            let outcome = self
                .renderer
                .as_mut()
                .map(|renderer| renderer.render(meshes));
            match outcome {
                Some(Ok(RenderOutcome::Presented)) => {
                    if should_refresh_input_region(
                        self.state.region_dirty,
                        &mut self.state.defer_region_refresh_once,
                    ) {
                        let hit_test_mask = HitTestMask::from_meshes(
                            meshes,
                            physical(self.state.placement.width, self.state.placement.scale),
                            physical(self.state.placement.height, self.state.placement.scale),
                        );
                        if self.state.hit_test_mask == hit_test_mask && !self.state.region_dirty {
                            return;
                        }
                        if apply_input_region(self.hwnd, &hit_test_mask).is_err() {
                            self.fail_surface();
                        } else {
                            self.state.hit_test_mask = hit_test_mask;
                            self.state.region_dirty = false;
                        }
                    }
                }
                Some(Ok(RenderOutcome::Skipped)) | None => {}
                Some(Err(failure)) => {
                    self.renderer = None;
                    self.gpu_failure = Some(gpu_info(failure));
                }
            }
        }

        fn fail_surface(&mut self) {
            self.renderer = None;
            self.gpu_failure = Some(GpuFailInfo {
                reason: GpuFailReason::Surface,
            });
            self.state.region_failed = true;
            self.state.hidden = true;
            // SAFETY: hwnd is live and hiding it prevents a stale region from
            // intercepting desktop input after native region failure.
            unsafe { ShowWindow(self.hwnd, SW_HIDE) };
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
                self.state.placement.width = logical_width;
                self.state.placement.height = logical_height;
                // WM_SIZE already updates placement. The renderer compares its
                // own physical extent, including changes of DPI at equal DIPs.
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(width, height);
                }
                if (
                    self.state.hit_test_mask.width(),
                    self.state.hit_test_mask.height(),
                ) != (width, height)
                {
                    self.state.region_dirty = true;
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
        hit_test_mask: HitTestMask,
        region_dirty: bool,
        defer_region_refresh_once: bool,
        region_failed: bool,
        dpi_resize_in_progress: bool,
    }

    /// Alpha-region rasterization walks every rendered triangle. Refreshing it
    /// on alternate presented frames caps pointer-region lag at roughly 65 ms
    /// while leaving the visual presentation cadence unchanged. Dirty geometry
    /// (startup, resize, or DPI transition) always refreshes immediately.
    fn should_refresh_input_region(region_dirty: bool, defer_once: &mut bool) -> bool {
        if region_dirty {
            *defer_once = true;
            return true;
        }
        if *defer_once {
            *defer_once = false;
            return false;
        }
        *defer_once = true;
        true
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
                let grip = physical(32, placement.scale);
                // SAFETY: state pointer validity established above.
                let hit_test_mask = unsafe { &(*state).hit_test_mask };
                if hit_test_mask.contains_resize_grip(point.x, point.y, grip) {
                    return HTBOTTOMRIGHT as LRESULT;
                }
                if hit_test_mask.contains(point.x, point.y) {
                    return HTCAPTION as LRESULT;
                }
                return HTTRANSPARENT as LRESULT;
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
                    if !(*state).dpi_resize_in_progress {
                        (*state)
                            .events
                            .push_back(LocalUiFact::Resize { width, height });
                    }
                    // SetWindowRgn can fail transiently while Windows is in a
                    // DPI/interactive-size transaction. Keep the last
                    // displayed silhouette (automatically clipped to the new
                    // HWND bounds) and force replacement only after the next
                    // actual presentation. A skipped frame must not publish a
                    // region for pixels that were never shown.
                    (*state).region_dirty = true;
                }
                return 0;
            }
            WM_DPICHANGED if !state.is_null() => {
                let suggested = lparam as *const RECT;
                if !suggested.is_null() {
                    // SAFETY: WM_DPICHANGED lparam is a suggested RECT.
                    let rect = unsafe { *suggested };
                    let dpi = (wparam as u32 & 0xffff).max(96);
                    // Keep the user-selected logical extent stable across
                    // monitors. Reusing the suggested physical extent and
                    // then reporting its WM_SIZE as a user resize compounds
                    // the scale factor on each 100%↔125% transition.
                    // SAFETY: state pointer validity was established above.
                    let (width, height) = unsafe {
                        (
                            physical((*state).placement.width, dpi as f32 / 96.0),
                            physical((*state).placement.height, dpi as f32 / 96.0),
                        )
                    };
                    // SAFETY: hwnd is live and rectangle is compositor supplied.
                    unsafe {
                        (*state).placement.scale = dpi as f32 / 96.0;
                        (*state).region_dirty = true;
                        (*state).dpi_resize_in_progress = true;
                        SetWindowPos(
                            hwnd,
                            std::ptr::null_mut(),
                            rect.left,
                            rect.top,
                            i32::try_from(width).unwrap_or(i32::MAX),
                            i32::try_from(height).unwrap_or(i32::MAX),
                            SWP_NOACTIVATE | SWP_NOZORDER,
                        );
                        (*state).dpi_resize_in_progress = false;
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

    fn apply_input_region(hwnd: HWND, mask: &HitTestMask) -> Result<(), String> {
        let region = create_input_region(mask)?;
        // SAFETY: hwnd is live and region is a valid GDI region. On success
        // Windows takes ownership; on failure this process still owns it.
        if unsafe { SetWindowRgn(hwnd, region, 1) } == 0 {
            // SAFETY: SetWindowRgn failed, so ownership was not transferred.
            unsafe { DeleteObject(region as _) };
            return Err(String::from("failed to apply alpha-aware window region"));
        }
        Ok(())
    }

    fn create_input_region(mask: &HitTestMask) -> Result<HRGN, String> {
        create_region_from_rectangles(&mask.opaque_rectangles())
    }

    fn create_region_from_rectangles(rectangles: &[[u32; 4]]) -> Result<HRGN, String> {
        if rectangles.is_empty() {
            // SAFETY: coordinates describe a valid empty region.
            let region = unsafe { CreateRectRgn(0, 0, 0, 0) };
            return if region.is_null() {
                Err(String::from("failed to create empty window region"))
            } else {
                Ok(region)
            };
        }

        let native = rectangles
            .iter()
            .map(|rectangle| {
                Ok(RECT {
                    left: i32::try_from(rectangle[0])
                        .map_err(|_| String::from("window region exceeds i32"))?,
                    top: i32::try_from(rectangle[1])
                        .map_err(|_| String::from("window region exceeds i32"))?,
                    right: i32::try_from(rectangle[2])
                        .map_err(|_| String::from("window region exceeds i32"))?,
                    bottom: i32::try_from(rectangle[3])
                        .map_err(|_| String::from("window region exceeds i32"))?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let rect_bytes = native
            .len()
            .checked_mul(std::mem::size_of::<RECT>())
            .ok_or_else(|| String::from("window region data is too large"))?;
        let data_bytes = std::mem::size_of::<RGNDATAHEADER>()
            .checked_add(rect_bytes)
            .ok_or_else(|| String::from("window region data is too large"))?;
        let count = u32::try_from(native.len())
            .map_err(|_| String::from("too many window region rectangles"))?;
        let rect_bytes_u32 = u32::try_from(rect_bytes)
            .map_err(|_| String::from("window region data is too large"))?;
        let data_bytes_u32 = u32::try_from(data_bytes)
            .map_err(|_| String::from("window region data is too large"))?;
        let bounds = native.iter().fold(
            RECT {
                left: i32::MAX,
                top: i32::MAX,
                right: i32::MIN,
                bottom: i32::MIN,
            },
            |bounds, rectangle| RECT {
                left: bounds.left.min(rectangle.left),
                top: bounds.top.min(rectangle.top),
                right: bounds.right.max(rectangle.right),
                bottom: bounds.bottom.max(rectangle.bottom),
            },
        );
        let header = RGNDATAHEADER {
            dwSize: u32::try_from(std::mem::size_of::<RGNDATAHEADER>())
                .map_err(|_| String::from("window region header is too large"))?,
            iType: RDH_RECTANGLES,
            nCount: count,
            nRgnSize: rect_bytes_u32,
            rcBound: bounds,
        };
        let words = data_bytes.div_ceil(std::mem::size_of::<u64>());
        let mut storage = vec![0_u64; words];
        let data = storage.as_mut_ptr().cast::<u8>();
        // SAFETY: the u64 allocation is aligned for RGNDATAHEADER and RECT,
        // data_bytes reserves the header followed by every native rectangle,
        // and both destinations are non-overlapping initialized slots.
        unsafe {
            std::ptr::write(data.cast::<RGNDATAHEADER>(), header);
            let destination = data
                .add(std::mem::size_of::<RGNDATAHEADER>())
                .cast::<RECT>();
            std::ptr::copy_nonoverlapping(native.as_ptr(), destination, native.len());
        }
        // SAFETY: data points to a complete RGNDATAHEADER followed by nCount
        // RECT values for the duration of this call.
        let region =
            unsafe { ExtCreateRegion(std::ptr::null(), data_bytes_u32, data.cast::<RGNDATA>()) };
        if region.is_null() {
            Err(String::from("failed to create alpha-aware window region"))
        } else {
            Ok(region)
        }
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

    #[cfg(test)]
    mod tests {
        use super::*;
        use windows_sys::Win32::Graphics::Gdi::PtInRegion;

        #[test]
        fn native_region_contains_only_supplied_visible_runs() {
            let region = create_region_from_rectangles(&[[0, 0, 8, 4], [8, 4, 12, 8]])
                .expect("create region");
            // SAFETY: region is live until DeleteObject below.
            unsafe {
                assert_ne!(PtInRegion(region, 2, 2), 0);
                assert_ne!(PtInRegion(region, 10, 6), 0);
                assert_eq!(PtInRegion(region, 10, 2), 0);
                assert_eq!(PtInRegion(region, 2, 6), 0);
                assert_ne!(DeleteObject(region as _), 0);
            }
        }

        #[test]
        fn native_empty_region_owns_no_point() {
            let region = create_region_from_rectangles(&[]).expect("create empty region");
            // SAFETY: region is live until DeleteObject below.
            unsafe {
                assert_eq!(PtInRegion(region, 0, 0), 0);
                assert_ne!(DeleteObject(region as _), 0);
            }
        }

        #[test]
        fn invalid_native_rectangle_fails_before_ownership_transfer() {
            let result = create_region_from_rectangles(&[[0, 0, u32::MAX, 4]]);
            assert!(result.is_err());
        }

        #[test]
        fn input_region_refreshes_immediately_then_on_alternate_presented_frames() {
            let mut defer_once = false;
            assert!(should_refresh_input_region(true, &mut defer_once));
            assert!(defer_once);
            assert!(!should_refresh_input_region(false, &mut defer_once));
            assert!(!defer_once);
            assert!(should_refresh_input_region(false, &mut defer_once));
            assert!(defer_once);
            assert!(should_refresh_input_region(true, &mut defer_once));
            assert!(defer_once);
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
