//! Coarse X11 SHAPE: Bounding from visual geometry, Input from interaction.

use winit::raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use winit::window::Window;
use x11rb::protocol::shape::{self, SK as ShapeKind, SO as ShapeOp};
use x11rb::protocol::xproto::{ClipOrdering, Rectangle};
use x11rb::rust_connection::RustConnection;

use crate::interaction_controller::InteractionMode;
use crate::scene::{InteractionGeometry, VisualGeometry};

use super::rects_i32;

pub struct X11Shape {
    conn: RustConnection,
    window: u32,
    shape_available: bool,
    fallback: bool,
}

impl X11Shape {
    pub fn try_new(window: &Window) -> Option<Self> {
        let display = window.display_handle().ok()?.as_raw();
        let handle = window.window_handle().ok()?.as_raw();
        if !matches!(
            display,
            RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_)
        ) {
            return None;
        }
        let window_id = match handle {
            RawWindowHandle::Xlib(xlib) => u32::try_from(xlib.window).ok()?,
            RawWindowHandle::Xcb(xcb) => xcb.window.get(),
            _ => return None,
        };
        let (conn, _) = x11rb::connect(None).ok()?;
        let shape_available = shape::query_version(&conn)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some();
        if !shape_available {
            tracing::warn!("X11 SHAPE missing; window-wide fallback");
        }
        Some(Self {
            conn,
            window: window_id,
            shape_available,
            fallback: !shape_available,
        })
    }

    pub fn apply(
        &mut self,
        mode: InteractionMode,
        visual: &VisualGeometry,
        interaction: &InteractionGeometry,
    ) {
        if self.fallback || !self.shape_available {
            return;
        }
        if !self.set_kind(ShapeKind::BOUNDING, &rects_i32(&visual.rects)) {
            self.fallback = true;
            tracing::warn!("X11 Bounding SHAPE failed; window-wide fallback");
            return;
        }
        let input_rects = match mode {
            InteractionMode::Passive => Vec::new(),
            InteractionMode::Dragging | InteractionMode::UiFocused => rects_i32(&visual.rects),
            InteractionMode::Interactive => rects_i32(&interaction.rects),
        };
        if !self.set_kind(ShapeKind::INPUT, &input_rects) {
            self.fallback = true;
            tracing::warn!("X11 Input SHAPE failed; keeping cursor poll fallback");
        }
    }

    fn set_kind(&self, kind: ShapeKind, rects: &[(i32, i32, i32, i32)]) -> bool {
        let xrects: Vec<Rectangle> = rects
            .iter()
            .filter_map(|(x, y, w, h)| {
                Some(Rectangle {
                    x: i16::try_from(*x).ok()?,
                    y: i16::try_from(*y).ok()?,
                    width: u16::try_from(*w).ok()?,
                    height: u16::try_from(*h).ok()?,
                })
            })
            .collect();
        shape::rectangles(
            &self.conn,
            ShapeOp::SET,
            kind,
            ClipOrdering::UNSORTED,
            self.window,
            0,
            0,
            &xrects,
        )
        .is_ok()
    }
}
