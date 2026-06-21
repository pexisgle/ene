//! X11 fallback for the click-through story.
//!
//! Winit 0.30's `Window::set_cursor_hittest` is Windows-only, so
//! the click-through story needs a per-display-server
//! implementation on Linux. Wayland uses
//! `wl_surface::set_input_region`; X11 uses the shape extension
//! (`shape::rectangles(SK::Input, SO::Set, …)` — empty list =
//! pass-through) plus the EWMH `_NET_WM_STATE_SKIP_TASKBAR |
//! _SKIP_PAGER` ClientMessage so the character never steals
//! focus on alt-tab.
//!
//! The winit raw handles are only used to **detect** X11; the
//! actual socket is opened from `$DISPLAY` because winit's
//! `X11DisplayHandle::display` is a `NonNull<c_void>` opaque
//! pointer x11rb 0.13 cannot accept.
//!
//! Failure modes: `DISPLAY` unset / no server — `try_new` returns
//! `None`; shape extension absent — `set_input_rects` is a no-op
//! (logged once); atom intern fails — `try_new` returns `None`.

#[cfg(target_os = "linux")]
use std::sync::{Arc, Once};

#[cfg(target_os = "linux")]
use parking_lot::Mutex;
#[cfg(target_os = "linux")]
use winit::raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
};
#[cfg(target_os = "linux")]
use x11rb::protocol::shape::{self, SK as ShapeKind, SO as ShapeOp};
#[cfg(target_os = "linux")]
use x11rb::protocol::xproto::{
    Atom, ClientMessageData, ClientMessageEvent, ClipOrdering, ConnectionExt, EventMask, Rectangle,
    Window,
};
#[cfg(target_os = "linux")]
use x11rb::rust_connection::RustConnection;

/// X11 window id (the 32-bit XID the server hands out when
/// winit creates the window).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub struct X11WindowId(pub u32);

/// EWMH atoms interned once on `try_new`.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
struct X11Atoms {
    net_wm_state: Atom,
    skip_taskbar: Atom,
    skip_pager: Atom,
    wm_state_remove: u32,
    wm_state_add: u32,
}

#[cfg(target_os = "linux")]
impl X11Atoms {
    fn intern(conn: &RustConnection) -> Result<Self, x11rb::errors::ReplyError> {
        let intern = |name: &[u8]| -> Result<Atom, x11rb::errors::ReplyError> {
            let cookie = conn.intern_atom(false, name)?;
            let reply = cookie.reply()?;
            Ok(reply.atom)
        };
        Ok(Self {
            net_wm_state: intern(b"_NET_WM_STATE")?,
            skip_taskbar: intern(b"_NET_WM_STATE_SKIP_TASKBAR")?,
            skip_pager: intern(b"_NET_WM_STATE_SKIP_PAGER")?,
            wm_state_remove: 0,
            wm_state_add: 1,
        })
    }
}

/// What the click-through dispatcher should send to the X11
/// shape extension this frame. Pure enum so the state machine
/// is testable without a live X server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum X11Path {
    /// The cursor is on the character silhouette, or the
    /// window is opaque — receive input everywhere.
    Full,
    /// The window is transparent *and* the cursor is off the
    /// silhouette — pass the click through to the desktop
    /// (empty shape input region).
    Empty,
    /// The user is holding the F8 "freeze" hotkey — receive
    /// input regardless of the cursor (overrides transparency).
    Frozen,
}

impl X11Path {
    /// Pure decision function. Mirrors the Wayland branch in
    /// [`crate::platform::apply_linux_click_through`].
    pub fn decide(allows_input: bool, cursor_on_silhouette: bool, freeze_forced: bool) -> Self {
        if freeze_forced {
            X11Path::Frozen
        } else if allows_input && cursor_on_silhouette {
            X11Path::Full
        } else {
            X11Path::Empty
        }
    }
}

/// X11 display-server context. Cheap to clone the `Arc`; the
/// inner `Mutex` is the only point of contention with the
/// runtime's `about_to_wait` dispatch.
pub struct X11Context {
    conn: RustConnection,
    window: Window,
    atoms: X11Atoms,
    /// `true` once `shape::query_version` has succeeded. The
    /// extension is not guaranteed (very old X servers, some Xvfb
    /// configurations).
    shape_available: bool,
}

impl X11Context {
    /// Open an X11 connection, intern the EWMH atoms, and
    /// apply `_NET_WM_STATE_SKIP_TASKBAR | _SKIP_PAGER` on
    /// the winit window. Returns `None` if the runtime is
    /// not running under X11, no X server is reachable, or
    /// any of the interned atoms is not provided by the
    /// EWMH-compliant window manager.
    #[allow(dead_code)]
    pub fn try_new<W: HasWindowHandle + HasDisplayHandle>(window: &W) -> Option<Arc<Mutex<Self>>> {
        if !is_x11_window(window) {
            return None;
        }
        let (conn, _screen_num) = x11rb::connect(None).ok()?;
        let window_id = x11_window_id(window)?;
        let atoms = X11Atoms::intern(&conn).ok()?;
        if atoms.net_wm_state == 0 || atoms.skip_taskbar == 0 || atoms.skip_pager == 0 {
            return None;
        }
        let shape_available = shape::query_version(&conn)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some();
        let ctx = Self {
            conn,
            window: window_id,
            atoms,
            shape_available,
        };
        // Apply `_NET_WM_STATE_SKIP_TASKBAR | _SKIP_PAGER` on
        // construction so the window never shows up in the
        // taskbar / pager from the first frame.
        let arc = Arc::new(Mutex::new(ctx));
        arc.lock().set_skip_taskbar(true);
        Some(arc)
    }

    /// Update the X11 shape extension input region. Empty slice
    /// = pass-through (no pixel receives input).
    #[allow(dead_code)]
    pub fn set_input_rects(&mut self, rects: &[super::wayland_region::Rect]) {
        if !self.shape_available {
            return;
        }
        let xrects: Vec<Rectangle> = rects
            .iter()
            .map(|r| Rectangle {
                x: i16::try_from(r.x).unwrap_or(i16::MAX),
                y: i16::try_from(r.y).unwrap_or(i16::MAX),
                width: u16::try_from(r.w).unwrap_or(u16::MAX),
                height: u16::try_from(r.h).unwrap_or(u16::MAX),
            })
            .collect();
        // `shape::rectangles` is fire-and-forget (no reply).
        let _ = shape::rectangles(
            &self.conn,
            ShapeOp::SET,
            ShapeKind::INPUT,
            ClipOrdering::YX_SORTED,
            self.window,
            0,
            0,
            &xrects,
        );
    }

    /// Clear the shape input region (no input = full
    /// pass-through to the desktop).
    #[allow(dead_code)]
    pub fn clear_input(&mut self) {
        self.set_input_rects(&[]);
    }

    /// Apply or revoke `_NET_WM_STATE_SKIP_TASKBAR |
    /// _NET_WM_STATE_SKIP_PAGER`.
    fn set_skip_taskbar(&mut self, skip: bool) {
        static LOGGED_NOOP: Once = Once::new();
        if self.atoms.net_wm_state == 0 {
            LOGGED_NOOP.call_once(|| {
                tracing::warn!(
                    target: "ene.linux.x11",
                    "X11 _NET_WM_STATE atom not interned; skip-taskbar request is a no-op"
                );
            });
            return;
        }
        let action = if skip {
            self.atoms.wm_state_add
        } else {
            self.atoms.wm_state_remove
        };
        let event = ClientMessageEvent {
            response_type: x11rb::protocol::xproto::CLIENT_MESSAGE_EVENT,
            format: 32,
            sequence: 0,
            window: self.window,
            type_: self.atoms.net_wm_state,
            data: ClientMessageData::from([
                action,
                #[allow(clippy::useless_conversion)]
                // `Atom: From<Atom> for u32`; explicit makes the byte order obvious.
                u32::from(self.atoms.skip_taskbar),
                #[allow(clippy::useless_conversion)]
                u32::from(self.atoms.skip_pager),
                1, // source indication: normal application
                0,
            ]),
        };
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        let _ = x11rb::protocol::xproto::send_event(&self.conn, true, self.window, mask, event);
    }
}

#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub fn is_x11_window<W: HasWindowHandle + HasDisplayHandle>(window: &W) -> bool {
    let Ok(display) = window.display_handle() else {
        return false;
    };
    matches!(
        display.as_raw(),
        RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_)
    )
}

/// X11 window id from the winit raw window handle, or `None`
/// if the window is not X11.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub fn x11_window_id<W: HasWindowHandle + HasDisplayHandle>(window: &W) -> Option<Window> {
    let win = window.window_handle().ok()?.as_raw();
    match win {
        RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
        RawWindowHandle::Xlib(handle) => Some(handle.window as u32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x11_path_decide_truth_table() {
        // `freeze_forced` always wins.
        assert_eq!(
            X11Path::decide(false, false, true),
            X11Path::Frozen,
            "freeze overrides transparent + off-silhouette"
        );
        assert_eq!(
            X11Path::decide(true, true, true),
            X11Path::Frozen,
            "freeze overrides silhouette"
        );
        // No freeze, normal hit-test.
        assert_eq!(
            X11Path::decide(true, true, false),
            X11Path::Full,
            "cursor on silhouette: receive input"
        );
        assert_eq!(
            X11Path::decide(false, true, false),
            X11Path::Empty,
            "transparent window blocks input even on silhouette"
        );
        assert_eq!(
            X11Path::decide(true, false, false),
            X11Path::Empty,
            "cursor off silhouette: pass through"
        );
        assert_eq!(
            X11Path::decide(false, false, false),
            X11Path::Empty,
            "transparent + off: pass through"
        );
    }

    #[test]
    fn x11_path_decide_freeze_does_not_depend_on_allows_input() {
        // `freeze_forced` short-circuits regardless of the
        // other two args. Test the four corners.
        for allows_input in [true, false] {
            for cursor_on_silhouette in [true, false] {
                assert_eq!(
                    X11Path::decide(allows_input, cursor_on_silhouette, true),
                    X11Path::Frozen,
                    "freeze must win for ({allows_input}, {cursor_on_silhouette}, true)"
                );
            }
        }
    }

    #[test]
    fn x11_window_id_carries_u32() {
        let id = X11WindowId(0xdead_beef);
        assert_eq!(id.0, 0xdead_beef);
        let copy = id;
        assert_eq!(id, copy);
    }

    #[test]
    fn x11_atoms_struct_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<X11Atoms>();
    }

    #[test]
    fn x11_path_enum_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<X11Path>();
    }
}
