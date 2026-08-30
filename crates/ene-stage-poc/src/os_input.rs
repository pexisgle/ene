//! OS-level click-through / input-region backend for Experiment B.

use std::time::Duration;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use winit::window::Window;

use crate::input::ScreenRect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayServer {
    Windows,
    X11,
    Wayland,
    Unknown,
}

impl DisplayServer {
    #[must_use]
    pub fn detect(window: &Window) -> Self {
        let Ok(display) = window.display_handle() else {
            return Self::Unknown;
        };
        let Ok(win) = window.window_handle() else {
            return Self::Unknown;
        };
        match (display.as_raw(), win.as_raw()) {
            (RawDisplayHandle::Windows(_), RawWindowHandle::Win32(_)) => Self::Windows,
            (RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_), _) => Self::X11,
            (RawDisplayHandle::Wayland(_), RawWindowHandle::Wayland(_)) => Self::Wayland,
            _ => Self::Unknown,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::X11 => "x11",
            Self::Wayland => "wayland",
            Self::Unknown => "unknown",
        }
    }
}

/// Platform input-region contract for a future `StageWindow`.
pub trait StageInputRegion {
    fn set_passthrough(&mut self, enabled: bool);
    fn update_input_region(&mut self, rects: &[ScreenRect]);
    fn display_server(&self) -> DisplayServer;
    fn supports_partial_region(&self) -> bool;
}

pub struct NativeInputRegion {
    window: std::sync::Arc<Window>,
    server: DisplayServer,
    last_passthrough: Option<bool>,
    os_update_count: u32,
    last_os_update: Duration,
    #[cfg(target_os = "linux")]
    x11: Option<X11Shape>,
    #[cfg(target_os = "linux")]
    wayland: Option<WaylandRegion>,
}

impl NativeInputRegion {
    #[must_use]
    pub fn attach(window: std::sync::Arc<Window>) -> Self {
        let server = DisplayServer::detect(window.as_ref());
        #[cfg(target_os = "linux")]
        let x11 = X11Shape::try_new(window.as_ref());
        #[cfg(target_os = "linux")]
        let wayland = WaylandRegion::try_new(window.as_ref());
        tracing::info!(
            server = server.name(),
            partial = match server {
                DisplayServer::X11 | DisplayServer::Wayland | DisplayServer::Windows => true,
                DisplayServer::Unknown => false,
            },
            "stage input region backend"
        );
        Self {
            window,
            server,
            last_passthrough: None,
            os_update_count: 0,
            last_os_update: Duration::ZERO,
            #[cfg(target_os = "linux")]
            x11,
            #[cfg(target_os = "linux")]
            wayland,
        }
    }

    #[must_use]
    pub const fn os_update_count(&self) -> u32 {
        self.os_update_count
    }

    #[must_use]
    pub const fn last_os_update(&self) -> Duration {
        self.last_os_update
    }

    pub fn debug_dump(&mut self, tag: &str) {
        #[cfg(target_os = "linux")]
        if let Some(x11) = self.x11.as_mut() {
            x11.dump(tag);
        }
        #[cfg(target_os = "linux")]
        if self.wayland.is_some() {
            tracing::info!(tag, "wayland input region attached (native wl_surface)");
        }
        let _ = tag;
    }
}

impl StageInputRegion for NativeInputRegion {
    fn set_passthrough(&mut self, enabled: bool) {
        if self.last_passthrough == Some(enabled) {
            return;
        }
        self.last_passthrough = Some(enabled);
        if let Err(err) = self.window.set_cursor_hittest(!enabled) {
            tracing::debug!(error = %err, "set_cursor_hittest unsupported");
        }
        #[cfg(target_os = "windows")]
        windows::set_ws_ex_transparent(self.window.as_ref(), enabled);
        if enabled {
            self.update_input_region(&[]);
        }
    }

    fn update_input_region(&mut self, rects: &[ScreenRect]) {
        let empty = rects.is_empty();
        if let Err(err) = self.window.set_cursor_hittest(!empty) {
            tracing::debug!(error = %err, "set_cursor_hittest unsupported");
        }
        let started = std::time::Instant::now();
        #[cfg(target_os = "windows")]
        windows::set_window_region(self.window.as_ref(), rects);
        #[cfg(target_os = "linux")]
        if let Some(x11) = self.x11.as_mut() {
            x11.set_rects(rects);
        }
        #[cfg(target_os = "linux")]
        if let Some(wayland) = self.wayland.as_mut() {
            wayland.set_rects(rects);
        }
        self.last_os_update = started.elapsed();
        self.os_update_count = self.os_update_count.saturating_add(1);
        self.last_passthrough = Some(empty);
    }

    fn display_server(&self) -> DisplayServer {
        self.server
    }

    fn supports_partial_region(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.x11.is_some() || self.wayland.is_some()
        }
        #[cfg(target_os = "windows")]
        {
            true
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            false
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Gdi::{
        CombineRgn, CreateRectRgn, DeleteObject, RGN_OR, SetWindowRgn,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_LAYERED, WS_EX_TRANSPARENT,
    };
    use winit::window::Window;

    use crate::input::ScreenRect;

    fn hwnd(window: &Window) -> Option<HWND> {
        let handle = window.window_handle().ok()?.as_raw();
        match handle {
            RawWindowHandle::Win32(win) => Some(win.hwnd.get() as HWND),
            _ => None,
        }
    }

    pub fn set_ws_ex_transparent(window: &Window, enabled: bool) {
        let Some(hwnd) = hwnd(window) else {
            return;
        };
        // SAFETY: HWND comes from the live winit window.
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        let next = if enabled {
            style | (WS_EX_LAYERED as isize) | (WS_EX_TRANSPARENT as isize)
        } else {
            style & !(WS_EX_TRANSPARENT as isize)
        };
        // SAFETY: same HWND; only EXSTYLE bits change.
        let _ = unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next) };
    }

    pub fn set_window_region(window: &Window, rects: &[ScreenRect]) {
        let Some(hwnd) = hwnd(window) else {
            return;
        };
        if rects.is_empty() {
            // Empty region: no pixels receive input.
            // SAFETY: HWND is valid; passing null restores the default region
            // after we also mark the window click-through via WS_EX_TRANSPARENT.
            let _ = unsafe { SetWindowRgn(hwnd, std::ptr::null_mut(), 1) };
            return;
        }
        let mut combined = std::ptr::null_mut();
        for rect in rects {
            let Some((x, y, w, h)) = rect.to_i32() else {
                continue;
            };
            // SAFETY: GDI region creation is a pure allocation.
            let piece = unsafe { CreateRectRgn(x, y, x + w, y + h) };
            if combined.is_null() {
                combined = piece;
            } else {
                // SAFETY: both regions are valid GDI objects from CreateRectRgn.
                let _ = unsafe { CombineRgn(combined, combined, piece, RGN_OR) };
                let _ = unsafe { DeleteObject(piece) };
            }
        }
        if !combined.is_null() {
            // SAFETY: SetWindowRgn takes ownership of `combined`.
            let _ = unsafe { SetWindowRgn(hwnd, combined, 1) };
        }
    }
}

#[cfg(target_os = "linux")]
struct X11Shape {
    conn: x11rb::rust_connection::RustConnection,
    window: u32,
    root: Option<u32>,
    frame: Option<u32>,
    target: X11Target,
    kinds: X11ShapeKinds,
    shape_available: bool,
    logged_shape: bool,
    logged_tree: bool,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum X11Target {
    Client,
    Frame,
    Both,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum X11ShapeKinds {
    Input,
    Bounding,
    Both,
}

#[cfg(target_os = "linux")]
impl X11Target {
    fn from_env() -> Self {
        match std::env::var("ENE_STAGE_POC_X11_TARGET")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "frame" => Self::Frame,
            "both" => Self::Both,
            _ => Self::Client,
        }
    }
}

#[cfg(target_os = "linux")]
impl X11ShapeKinds {
    fn from_env() -> Self {
        match std::env::var("ENE_STAGE_POC_SHAPE_KIND")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "bounding" => Self::Bounding,
            "both" => Self::Both,
            _ => Self::Input,
        }
    }

    fn kinds(self) -> &'static [x11rb::protocol::shape::SK] {
        use x11rb::protocol::shape::SK;
        match self {
            Self::Input => &[SK::INPUT],
            Self::Bounding => &[SK::BOUNDING],
            Self::Both => &[SK::INPUT, SK::BOUNDING],
        }
    }
}

#[cfg(target_os = "linux")]
impl X11Shape {
    fn try_new(window: &Window) -> Option<Self> {
        let display = window.display_handle().ok()?.as_raw();
        if !matches!(
            display,
            RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_)
        ) {
            return None;
        }
        let win = window.window_handle().ok()?.as_raw();
        let xid = match win {
            RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok()?,
            RawWindowHandle::Xcb(h) => h.window.get(),
            _ => return None,
        };
        let (conn, _) = x11rb::connect(None).ok()?;
        let version = x11rb::protocol::shape::query_version(&conn)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        let shape_available = version.is_some();
        let xfixes = x11rb::protocol::xfixes::query_version(&conn, 5, 0)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        let target = X11Target::from_env();
        let kinds = X11ShapeKinds::from_env();
        tracing::info!(
            shape_available,
            xid,
            major = version.as_ref().map(|v| v.major_version),
            minor = version.as_ref().map(|v| v.minor_version),
            xfixes_major = xfixes.as_ref().map(|v| v.major_version),
            xfixes_minor = xfixes.as_ref().map(|v| v.minor_version),
            ?target,
            ?kinds,
            "X11 shape input region"
        );
        let mut this = Self {
            conn,
            window: xid,
            root: None,
            frame: None,
            target,
            kinds,
            shape_available,
            logged_shape: false,
            logged_tree: false,
        };
        this.request_bypass_compositor();
        this.refresh_tree();
        this.log_wm();
        Some(this)
    }

    fn request_bypass_compositor(&self) {
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode};
        let Ok(cookie) = self.conn.intern_atom(false, b"_NET_WM_BYPASS_COMPOSITOR") else {
            return;
        };
        let Ok(reply) = cookie.reply() else {
            return;
        };
        match x11rb::wrapper::ConnectionExt::change_property32(
            &self.conn,
            PropMode::REPLACE,
            self.window,
            reply.atom,
            AtomEnum::CARDINAL,
            &[1_u32],
        ) {
            Ok(prop) => {
                if let Err(err) = prop.check() {
                    tracing::debug!(error = %err, "bypass compositor property failed");
                }
            }
            Err(err) => tracing::debug!(error = %err, "bypass compositor property send failed"),
        }
        drop(x11rb::connection::Connection::flush(&self.conn));
        tracing::info!("requested _NET_WM_BYPASS_COMPOSITOR=1");
    }

    fn intern(&self, name: &[u8]) -> Option<u32> {
        use x11rb::protocol::xproto::ConnectionExt;
        let cookie = self.conn.intern_atom(false, name).ok()?;
        Some(cookie.reply().ok()?.atom)
    }

    fn log_wm(&self) {
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
        let Some(root) = self.root else {
            return;
        };
        let Some(check) = self.intern(b"_NET_SUPPORTING_WM_CHECK") else {
            return;
        };
        let Ok(cookie) = self
            .conn
            .get_property(false, root, check, AtomEnum::WINDOW, 0, 1)
        else {
            return;
        };
        let Ok(reply) = cookie.reply() else {
            return;
        };
        let Some(wm_win) = reply.value32().and_then(|mut v| v.next()) else {
            tracing::info!("_NET_SUPPORTING_WM_CHECK missing");
            return;
        };
        let name_atom = self.intern(b"_NET_WM_NAME");
        let utf8 = self.intern(b"UTF8_STRING");
        let wm_name = match (name_atom, utf8) {
            (Some(name), Some(utf8)) => self
                .conn
                .get_property(false, wm_win, name, utf8, 0, 256)
                .ok()
                .and_then(|c| c.reply().ok())
                .and_then(|prop| {
                    if prop.value.is_empty() {
                        None
                    } else {
                        String::from_utf8(prop.value).ok()
                    }
                }),
            _ => None,
        };
        tracing::info!(wm_win, wm_name = wm_name.as_deref(), "X11 supporting WM");
    }

    fn refresh_tree(&mut self) {
        use x11rb::protocol::xproto::ConnectionExt;
        let Ok(cookie) = self.conn.query_tree(self.window) else {
            return;
        };
        let Ok(tree) = cookie.reply() else {
            return;
        };
        self.root = Some(tree.root);
        self.frame = if tree.parent == 0 || tree.parent == tree.root {
            None
        } else {
            Some(tree.parent)
        };
        let attrs = self
            .conn
            .get_window_attributes(self.window)
            .ok()
            .and_then(|c| c.reply().ok());
        if !self.logged_tree {
            self.logged_tree = true;
            tracing::info!(
                client = self.window,
                parent = tree.parent,
                root = tree.root,
                children = tree.children.len(),
                frame = self.frame,
                override_redirect = attrs.as_ref().map(|a| a.override_redirect),
                map_state = attrs.as_ref().map(|a| format!("{:?}", a.map_state)),
                "XQueryTree"
            );
        }
    }

    fn destinations(&self) -> Vec<u32> {
        match self.target {
            X11Target::Client => vec![self.window],
            X11Target::Frame => self.frame.into_iter().collect(),
            X11Target::Both => {
                let mut out = vec![self.window];
                if let Some(frame) = self.frame {
                    out.push(frame);
                }
                out
            }
        }
    }

    fn set_rects(&mut self, rects: &[ScreenRect]) {
        if !self.shape_available {
            return;
        }
        self.refresh_tree();
        use x11rb::protocol::shape::{self, SO as ShapeOp};
        use x11rb::protocol::xproto::{ClipOrdering, Rectangle};
        let xrects: Vec<Rectangle> = rects
            .iter()
            .filter_map(|rect| rect.to_i32())
            .filter_map(|(x, y, w, h)| {
                Some(Rectangle {
                    x: i16::try_from(x).ok()?,
                    y: i16::try_from(y).ok()?,
                    width: u16::try_from(w).ok()?,
                    height: u16::try_from(h).ok()?,
                })
            })
            .collect();
        for dest in self.destinations() {
            for kind in self.kinds.kinds() {
                match shape::rectangles(
                    &self.conn,
                    ShapeOp::SET,
                    *kind,
                    ClipOrdering::YX_SORTED,
                    dest,
                    0,
                    0,
                    &xrects,
                ) {
                    Ok(cookie) => {
                        if let Err(err) = cookie.check() {
                            tracing::warn!(error = %err, dest, ?kind, "x11 shape check failed");
                        }
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, dest, ?kind, "x11 shape rectangles failed");
                    }
                }
            }
        }
        drop(x11rb::connection::Connection::flush(&self.conn));
        if !self.logged_shape {
            self.dump("after-set");
            self.logged_shape = true;
        }
    }

    fn dump(&mut self, tag: &str) {
        self.refresh_tree();
        use x11rb::protocol::shape::{self, SK};
        let mut windows = vec![("client", self.window)];
        if let Some(frame) = self.frame {
            windows.push(("frame", frame));
        }
        for (role, dest) in windows {
            for kind in [SK::INPUT, SK::BOUNDING] {
                match shape::get_rectangles(&self.conn, dest, kind) {
                    Ok(cookie) => match cookie.reply() {
                        Ok(reply) => {
                            let rects: Vec<String> = reply
                                .rectangles
                                .iter()
                                .map(|r| format!("{}x{}+{}+{}", r.width, r.height, r.x, r.y))
                                .collect();
                            tracing::info!(
                                tag,
                                role,
                                dest,
                                kind = format!("{kind:?}"),
                                n = reply.rectangles.len(),
                                rects = rects.join(","),
                                "XShapeGetRectangles"
                            );
                        }
                        Err(err) => tracing::debug!(error = %err, dest, "shape reply failed"),
                    },
                    Err(err) => tracing::debug!(error = %err, dest, "shape get failed"),
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
struct WaylandRegion {
    connection: wayland_client::Connection,
    compositor: wayland_client::protocol::wl_compositor::WlCompositor,
    queue_handle: wayland_client::QueueHandle<WaylandData>,
    event_queue: wayland_client::EventQueue<WaylandData>,
    surface: wayland_client::protocol::wl_surface::WlSurface,
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct WaylandData {
    compositor: Option<wayland_client::protocol::wl_compositor::WlCompositor>,
}

#[cfg(target_os = "linux")]
impl WaylandRegion {
    fn try_new(window: &Window) -> Option<Self> {
        use wayland_client::protocol::wl_surface::WlSurface;
        use wayland_client::{Connection, Proxy};

        let display = window.display_handle().ok()?.as_raw();
        let win = window.window_handle().ok()?.as_raw();
        let RawDisplayHandle::Wayland(wl_display) = display else {
            return None;
        };
        let RawWindowHandle::Wayland(wl_window) = win else {
            return None;
        };
        // SAFETY: winit's Wayland display pointer stays alive with the window.
        // Guest mode does not close the display when Connection is dropped.
        let connection = unsafe {
            let backend = wayland_client::backend::Backend::from_foreign_display(
                wl_display.display.as_ptr().cast(),
            );
            Connection::from_backend(backend)
        };
        let mut event_queue = connection.new_event_queue();
        let queue_handle = event_queue.handle();
        let _registry = connection.display().get_registry(&queue_handle, ());
        let mut data = WaylandData::default();
        if event_queue.roundtrip(&mut data).is_err() {
            return None;
        }
        let compositor = data.compositor.clone()?;
        let raw_surface = wl_window.surface.as_ptr();
        // SAFETY: pointer is winit's live wl_surface.
        let object_id = unsafe {
            wayland_client::backend::ObjectId::from_ptr(
                <WlSurface as Proxy>::interface(),
                raw_surface.cast(),
            )
        }
        .ok()?;
        let surface = Proxy::from_id(&connection, object_id).ok()?;
        tracing::info!("Wayland wl_surface input region attached");
        Some(Self {
            connection,
            compositor,
            queue_handle,
            event_queue,
            surface,
        })
    }

    fn set_rects(&mut self, rects: &[ScreenRect]) {
        let mut data = WaylandData::default();
        let pending = self.event_queue.dispatch_pending(&mut data);
        drop(pending);
        let _ = &self.connection;
        let region = self.compositor.create_region(&self.queue_handle, ());
        for rect in rects {
            let Some((x, y, w, h)) = rect.to_i32() else {
                continue;
            };
            region.add(x, y, w, h);
        }
        self.surface.set_input_region(Some(&region));
        self.surface.commit();
        drop(self.connection.flush());
    }
}

#[cfg(target_os = "linux")]
impl wayland_client::Dispatch<wayland_client::protocol::wl_registry::WlRegistry, ()>
    for WaylandData
{
    fn event(
        state: &mut Self,
        registry: &wayland_client::protocol::wl_registry::WlRegistry,
        event: wayland_client::protocol::wl_registry::Event,
        (): &(),
        _conn: &wayland_client::Connection,
        qh: &wayland_client::QueueHandle<Self>,
    ) {
        if let wayland_client::protocol::wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == "wl_compositor"
        {
            state.compositor = Some(
                registry.bind::<wayland_client::protocol::wl_compositor::WlCompositor, _, _>(
                    name,
                    version.min(4),
                    qh,
                    (),
                ),
            );
        }
    }
}

#[cfg(target_os = "linux")]
impl wayland_client::Dispatch<wayland_client::protocol::wl_compositor::WlCompositor, ()>
    for WaylandData
{
    fn event(
        _state: &mut Self,
        _proxy: &wayland_client::protocol::wl_compositor::WlCompositor,
        _event: wayland_client::protocol::wl_compositor::Event,
        (): &(),
        _conn: &wayland_client::Connection,
        _qh: &wayland_client::QueueHandle<Self>,
    ) {
    }
}

#[cfg(target_os = "linux")]
impl wayland_client::Dispatch<wayland_client::protocol::wl_region::WlRegion, ()> for WaylandData {
    fn event(
        _state: &mut Self,
        _proxy: &wayland_client::protocol::wl_region::WlRegion,
        _event: wayland_client::protocol::wl_region::Event,
        (): &(),
        _conn: &wayland_client::Connection,
        _qh: &wayland_client::QueueHandle<Self>,
    ) {
    }
}

#[cfg(target_os = "linux")]
impl wayland_client::Dispatch<wayland_client::protocol::wl_surface::WlSurface, ()> for WaylandData {
    fn event(
        _state: &mut Self,
        _proxy: &wayland_client::protocol::wl_surface::WlSurface,
        _event: wayland_client::protocol::wl_surface::Event,
        (): &(),
        _conn: &wayland_client::Connection,
        _qh: &wayland_client::QueueHandle<Self>,
    ) {
    }
}
