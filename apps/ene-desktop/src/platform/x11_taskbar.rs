//! PR5.4 / PR-LX.5: X11 fallback for the click-through story.
//!
//! Winit 0.30's `Window::set_cursor_hittest` is a Windows-only
//! no-op, so the click-through story that PR5.2 shipped for
//! the Windows desktop (via `WS_EX_TRANSPARENT`) needs a
//! per-display-server implementation on Linux. Wayland uses
//! `wl_surface::set_input_region` (PR5.3 / LX.2). On X11 the
//! equivalent primitives are:
//!
//! - **shape extension input region** — an X11 `Rectangle` set
//!   per window that tells the X server which pixels receive
//!   pointer events. `shape::rectangles(SK::Input, SO::Set, …)`
//!   replaces the silhouette. Empty = no input (full
//!   pass-through).
//! - **`_NET_WM_STATE_SKIP_TASKBAR` / `_SKIP_PAGER`** (EWMH) —
//!   ClientMessage on the window that hides it from the
//!   taskbar / pager so the character never steals focus when
//!   the user alt-tabs.
//!
//! The connection is established via [`x11rb::connect`], the
//! pure-Rust X11 client. The winit window's raw handles are
//! only inspected to **detect** the X11 display server
//! (`RawDisplayHandle::X11`); the actual socket is opened
//! from `$DISPLAY` because winit's `X11DisplayHandle::display`
//! is a `NonNull<c_void>` opaque pointer that x11rb 0.13 does
//! not accept.
//!
//! # LX.5 scope
//!
//! This module provides:
//!
//! - `X11Context::try_new<W>` — open a connection, intern the
//!   five EWMH atoms, and apply `_NET_WM_STATE_SKIP_TASKBAR |
//!   _SKIP_PAGER` once on construction.
//! - `set_input_rects(&[Rect])` — push a rectangle set to the
//!   shape extension. Empty list = pass-through.
//! - `clear_input()` — pass-through shorthand.
//! - `X11Path::decide(allows_input, cursor_on_silhouette, freeze_forced)`
//!   — pure helper used by the runtime to decide between
//!   `Full` / `Empty` / `Frozen`, unit-tested without an
//!   X server.
//!
//! # Failure modes
//!
//! - `DISPLAY` not set / no X server — `try_new` returns `None`
//!   and the runtime silently falls back to the
//!   no-click-through-on-Linux path.
//! - shape extension not present — `set_input_rects` is a
//!   no-op (logged once via `Once`).
//! - atom intern fails — `try_new` returns `None`.
//!
//! # Architecture
//!
//! The X11 connection lives behind a `parking_lot::Mutex` in
//! an `Arc`; the runtime holds the `Arc` on
//! `AppState::x11_ctx`. `RustConnection` is `Send + Sync`, so
//! the `Arc<Mutex<…>>` pattern is sound. The runtime calls
//! [`crate::platform::apply_linux_click_through`] every
//! `about_to_wait`; that function acquires the X11 lock and
//! pushes the current policy.

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
#[allow(dead_code)] // PR-LX.5: the public API is fixed here; consumers land in PR-LX.7.
pub struct X11WindowId(pub u32);

/// EWMH atoms interned once on `try_new`. Stored on the
/// context so we don't re-intern on every message.
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
/// shape extension this frame. A pure enum so the
/// `apply_linux_click_through` state machine is testable
/// without a live X server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // PR-LX.5: the public API is fixed here; consumers land in PR-LX.7.
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
    /// `true` once `shape::query_version` has succeeded on
    /// this connection. The shape extension is not guaranteed
    /// to exist (very old X servers, headless Xvfb in some
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
    #[allow(dead_code)] // `try_new` is consumed by the runtime in `Runtime::resumed`.
    pub fn try_new<W: HasWindowHandle + HasDisplayHandle>(window: &W) -> Option<Arc<Mutex<Self>>> {
        // 1. Probe the raw display handle for the X11
        //    flavor. On Wayland / Windows this is a no-op and
        //    we return `None`.
        if !is_x11_window(window) {
            return None;
        }

        // 2. Open the connection. `x11rb::connect(None)`
        //    reads the `$DISPLAY` env var. A missing server
        //    is a clean error path.
        let (conn, _screen_num) = x11rb::connect(None).ok()?;

        // 3. Recover the X11 window id from the winit raw
        //    handle. winit sets the `.window` field of
        //    `X11WindowHandle` to the XID the server
        //    assigned when the HWND-equivalent was created.
        let window_id = x11_window_id(window)?;

        // 4. Intern the EWMH atoms. If the WM doesn't
        //    support EWMH at all, the intern returns atom 0
        //    and we refuse to construct the context (the
        //    taskbar path is harmless but useless without a
        //    WM that reads it).
        let atoms = X11Atoms::intern(&conn).ok()?;
        if atoms.net_wm_state == 0 || atoms.skip_taskbar == 0 || atoms.skip_pager == 0 {
            return None;
        }

        // 5. Probe the shape extension. `query_version` is
        //    the canonical presence check; if the X server
        //    returns an error the extension is not
        //    advertised.
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

        // 6. Apply `_NET_WM_STATE_SKIP_TASKBAR | _SKIP_PAGER`
        //    on construction so the window never shows up
        //    in the taskbar / pager from the first frame.
        let arc = Arc::new(Mutex::new(ctx));
        arc.lock().set_skip_taskbar(true);
        Some(arc)
    }

    /// Update the X11 shape extension input region. Empty
    /// slice = pass-through (no pixel receives input).
    #[allow(dead_code)] // `set_input_rects` is consumed by the runtime's Linux dispatch.
    pub fn set_input_rects(&mut self, rects: &[(i32, i32, i32, i32)]) {
        if !self.shape_available {
            return;
        }
        let xrects: Vec<Rectangle> = rects
            .iter()
            .map(|&(x, y, w, h)| Rectangle {
                x: i16::try_from(x).unwrap_or(i16::MAX),
                y: i16::try_from(y).unwrap_or(i16::MAX),
                width: u16::try_from(w).unwrap_or(u16::MAX),
                height: u16::try_from(h).unwrap_or(u16::MAX),
            })
            .collect();
        // `shape::rectangles` is fire-and-forget on the X
        // server side (no reply); we ignore the cookie.
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
    #[allow(dead_code)] // `clear_input` is consumed by the runtime's Linux dispatch.
    pub fn clear_input(&mut self) {
        self.set_input_rects(&[]);
    }

    /// Apply or revoke `_NET_WM_STATE_SKIP_TASKBAR |
    /// _NET_WM_STATE_SKIP_PAGER`. The message is a
    /// ClientMessage on the window with `data.l[0] = 1
    /// (add) | 0 (remove)`, `data.l[1] = skip_taskbar`,
    /// `data.l[2] = skip_pager`, `data.l[3] = 1 (source
    /// indication)`, `data.l[4] = 0`.
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

/// Returns `true` if the winit window's raw display handle
/// resolves to X11. Pure, side-effect-free probe; used as the
/// first guard in [`X11Context::try_new`].
#[cfg(target_os = "linux")]
#[allow(dead_code)] // `is_x11_window` is consumed by the runtime's Linux dispatch.
pub fn is_x11_window<W: HasWindowHandle + HasDisplayHandle>(window: &W) -> bool {
    let Ok(display) = window.display_handle() else {
        return false;
    };
    matches!(display.as_raw(), RawDisplayHandle::Xcb(_))
}

/// Returns the X11 window id (XID) from the winit raw window
/// handle, or `None` if the window is not X11. The XID is the
/// `window` field of the `X11WindowHandle` struct cast to
/// `u32` (the X protocol type for window ids).
#[cfg(target_os = "linux")]
#[allow(dead_code)] // `x11_window_id` is consumed by the runtime's Linux dispatch.
pub fn x11_window_id<W: HasWindowHandle + HasDisplayHandle>(window: &W) -> Option<Window> {
    let win = window.window_handle().ok()?.as_raw();
    if let RawWindowHandle::Xcb(handle) = win {
        Some(handle.window.get())
    } else {
        None
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
        // Compile-time assertion: the `X11Atoms` field bag
        // must remain `Send + Sync` so the parent
        // `X11Context` can live behind a `parking_lot::Mutex`
        // and be shared across the winit thread and any
        // future background pump.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<X11Atoms>();
    }

    #[test]
    fn x11_path_enum_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<X11Path>();
    }
}
