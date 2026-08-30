//! Independent Bounding / Input SHAPE control for Experiment D2 (X11 only).

use crate::input::ScreenRect;

#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use crate::region::{EffectiveInput, classify_effective_input};

#[derive(Debug, Clone, Copy, Default)]
pub struct SplitCosts {
    pub bounding_sets: u32,
    pub input_sets: u32,
    pub combined_sets: u32,
    pub bounding_ns: u128,
    pub input_ns: u128,
    pub combined_ns: u128,
    pub get_ns: u128,
    pub get_n: u32,
    pub notifies: u32,
    pub input_notifies: u32,
    pub wm_resets: u32,
    pub reapplies: u32,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
pub struct ShapeSnapshot {
    pub client: u32,
    pub frame: Option<u32>,
    pub bounding_client: Vec<ScreenRect>,
    pub input_client: Vec<ScreenRect>,
    pub clip_client: Vec<ScreenRect>,
    pub bounding_frame: Vec<ScreenRect>,
    pub input_frame: Vec<ScreenRect>,
    pub clip_frame: Vec<ScreenRect>,
    pub client_geom: ScreenRect,
    pub frame_geom: Option<ScreenRect>,
    pub effective: EffectiveInput,
}

#[cfg(target_os = "linux")]
pub struct X11SplitShapes {
    conn: x11rb::rust_connection::RustConnection,
    window: u32,
    root: Option<u32>,
    frame: Option<u32>,
    shape_available: bool,
    requested_bounding: Vec<ScreenRect>,
    requested_input: Vec<ScreenRect>,
    pub costs: SplitCosts,
    last_reset: Option<Instant>,
    started: Instant,
    input_matches: bool,
}

#[cfg(not(target_os = "linux"))]
pub struct X11SplitShapes {
    pub costs: SplitCosts,
}

#[cfg(not(target_os = "linux"))]
impl X11SplitShapes {
    #[must_use]
    pub fn try_new(_window: &winit::window::Window) -> Option<Self> {
        None
    }
}

#[cfg(target_os = "linux")]
impl X11SplitShapes {
    pub fn try_new(window: &winit::window::Window) -> Option<Self> {
        use raw_window_handle::{
            HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
        };
        let display = window.display_handle().ok()?.as_raw();
        if !matches!(
            display,
            RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_)
        ) {
            return None;
        }
        let xid = match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok()?,
            RawWindowHandle::Xcb(h) => h.window.get(),
            _ => return None,
        };
        let (conn, _) = x11rb::connect(None).ok()?;
        let version = x11rb::protocol::shape::query_version(&conn)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        let xfixes = x11rb::protocol::xfixes::query_version(&conn, 5, 0)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        tracing::info!(
            xid,
            shape = version.is_some(),
            major = version.as_ref().map(|v| v.major_version),
            minor = version.as_ref().map(|v| v.minor_version),
            xfixes_major = xfixes.as_ref().map(|v| v.major_version),
            xfixes_minor = xfixes.as_ref().map(|v| v.minor_version),
            "X11 split Bounding/Input backend"
        );
        let mut this = Self {
            conn,
            window: xid,
            root: None,
            frame: None,
            shape_available: version.is_some(),
            requested_bounding: Vec::new(),
            requested_input: Vec::new(),
            costs: SplitCosts::default(),
            last_reset: None,
            started: Instant::now(),
            input_matches: true,
        };
        this.refresh_tree();
        this.select_shape_notify();
        this.log_wm();
        Some(this)
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
            println!("wm=unknown");
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
                .and_then(|prop| String::from_utf8(prop.value).ok()),
            _ => None,
        };
        println!(
            "wm_win=0x{wm_win:x} wm_name={}",
            wm_name.as_deref().unwrap_or("unknown")
        );
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
    }

    fn select_shape_notify(&self) {
        use x11rb::protocol::shape;
        if !self.shape_available {
            return;
        }
        for dest in self.destinations() {
            match shape::select_input(&self.conn, dest, true) {
                Ok(cookie) => {
                    if let Err(err) = cookie.check() {
                        tracing::warn!(error = %err, dest, "ShapeNotify select_input failed");
                    }
                }
                Err(err) => tracing::warn!(error = %err, dest, "ShapeNotify select send failed"),
            }
        }
        drop(x11rb::connection::Connection::flush(&self.conn));
    }

    fn destinations(&self) -> Vec<u32> {
        let mut out = vec![self.window];
        if let Some(frame) = self.frame {
            out.push(frame);
        }
        out
    }

    fn to_xrects(rects: &[ScreenRect]) -> Vec<x11rb::protocol::xproto::Rectangle> {
        use x11rb::protocol::xproto::Rectangle;
        rects
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
            .collect()
    }

    fn set_kind(&mut self, kind: x11rb::protocol::shape::SK, rects: &[ScreenRect]) -> Duration {
        use x11rb::protocol::shape::{self, SO as ShapeOp};
        use x11rb::protocol::xproto::ClipOrdering;
        let started = Instant::now();
        if !self.shape_available {
            return started.elapsed();
        }
        self.refresh_tree();
        let xrects = Self::to_xrects(rects);
        for dest in self.destinations() {
            match shape::rectangles(
                &self.conn,
                ShapeOp::SET,
                kind,
                ClipOrdering::YX_SORTED,
                dest,
                0,
                0,
                &xrects,
            ) {
                Ok(cookie) => {
                    if let Err(err) = cookie.check() {
                        tracing::warn!(error = %err, dest, ?kind, "split shape SET failed");
                    }
                }
                Err(err) => tracing::warn!(error = %err, dest, ?kind, "split shape send failed"),
            }
        }
        drop(x11rb::connection::Connection::flush(&self.conn));
        started.elapsed()
    }

    pub fn set_bounding(&mut self, rects: &[ScreenRect]) -> Duration {
        self.requested_bounding = rects.to_vec();
        let dt = self.set_kind(x11rb::protocol::shape::SK::BOUNDING, rects);
        self.costs.bounding_ns += dt.as_nanos();
        self.costs.bounding_sets = self.costs.bounding_sets.saturating_add(1);
        dt
    }

    pub fn set_input(&mut self, rects: &[ScreenRect]) -> Duration {
        self.requested_input = rects.to_vec();
        let dt = self.set_kind(x11rb::protocol::shape::SK::INPUT, rects);
        self.costs.input_ns += dt.as_nanos();
        self.costs.input_sets = self.costs.input_sets.saturating_add(1);
        dt
    }

    pub fn set_split(&mut self, bounding: &[ScreenRect], input: &[ScreenRect]) -> Duration {
        let started = Instant::now();
        let _b = self.set_bounding(bounding);
        let _i = self.set_input(input);
        let dt = started.elapsed();
        self.costs.combined_ns += dt.as_nanos();
        self.costs.combined_sets = self.costs.combined_sets.saturating_add(1);
        dt
    }

    fn read_kind(&mut self, dest: u32, kind: x11rb::protocol::shape::SK) -> Vec<ScreenRect> {
        use x11rb::protocol::shape;
        let started = Instant::now();
        let out = shape::get_rectangles(&self.conn, dest, kind)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| {
                reply
                    .rectangles
                    .iter()
                    .map(|rect| {
                        ScreenRect::new(
                            f32::from(rect.x),
                            f32::from(rect.y),
                            f32::from(rect.width),
                            f32::from(rect.height),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.costs.get_ns += started.elapsed().as_nanos();
        self.costs.get_n = self.costs.get_n.saturating_add(1);
        out
    }

    fn geom(&self, dest: u32) -> ScreenRect {
        use x11rb::protocol::xproto::ConnectionExt;
        self.conn
            .get_geometry(dest)
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or_else(
                || ScreenRect::new(0.0, 0.0, 0.0, 0.0),
                |g| {
                    ScreenRect::new(
                        f32::from(g.x),
                        f32::from(g.y),
                        f32::from(g.width),
                        f32::from(g.height),
                    )
                },
            )
    }

    pub fn snapshot(&mut self) -> ShapeSnapshot {
        self.refresh_tree();
        let bounding_client = self.read_kind(self.window, x11rb::protocol::shape::SK::BOUNDING);
        let input_client = self.read_kind(self.window, x11rb::protocol::shape::SK::INPUT);
        let clip_client = self.read_kind(self.window, x11rb::protocol::shape::SK::CLIP);
        let (bounding_frame, input_frame, clip_frame, frame_geom) = if let Some(frame) = self.frame
        {
            (
                self.read_kind(frame, x11rb::protocol::shape::SK::BOUNDING),
                self.read_kind(frame, x11rb::protocol::shape::SK::INPUT),
                self.read_kind(frame, x11rb::protocol::shape::SK::CLIP),
                Some(self.geom(frame)),
            )
        } else {
            (Vec::new(), Vec::new(), Vec::new(), None)
        };
        let client_geom = self.geom(self.window);
        let effective = classify_effective_input(
            &input_client,
            &bounding_client,
            client_geom,
            frame_geom,
            &self.requested_input,
        );
        ShapeSnapshot {
            client: self.window,
            frame: self.frame,
            bounding_client,
            input_client,
            clip_client,
            bounding_frame,
            input_frame,
            clip_frame,
            client_geom,
            frame_geom,
            effective,
        }
    }

    pub fn dump(&mut self, tag: &str) {
        let snap = self.snapshot();
        println!("=== shape dump {tag} ===");
        println!("client=0x{:x}", snap.client);
        match snap.frame {
            Some(frame) => println!("frame=0x{frame:x}"),
            None => println!("frame=none"),
        }
        println!(
            "client_geom={} frame_geom={}",
            fmt_rects(std::slice::from_ref(&snap.client_geom)),
            snap.frame_geom.as_ref().map_or_else(
                || "(none)".to_owned(),
                |r| fmt_rects(std::slice::from_ref(r))
            )
        );
        println!("Bounding(client): {}", fmt_rects(&snap.bounding_client));
        println!("Input(client): {}", fmt_rects(&snap.input_client));
        println!("Clip(client): {}", fmt_rects(&snap.clip_client));
        println!("Bounding(frame): {}", fmt_rects(&snap.bounding_frame));
        println!("Input(frame): {}", fmt_rects(&snap.input_frame));
        println!("Clip(frame): {}", fmt_rects(&snap.clip_frame));
        println!("effective_input_shape={:?}", snap.effective);
        println!(
            "requested Bounding={} Input={}",
            fmt_rects(&self.requested_bounding),
            fmt_rects(&self.requested_input)
        );
        tracing::info!(
            tag,
            client = snap.client,
            frame = snap.frame,
            effective = ?snap.effective,
            "X11 split shape dump"
        );
    }

    pub fn poll_notifies(&mut self) {
        use x11rb::connection::Connection;
        loop {
            match self.conn.poll_for_event() {
                Ok(Some(event)) => self.on_event(&event),
                Ok(None) => break,
                Err(err) => {
                    tracing::debug!(error = %err, "shape poll failed");
                    break;
                }
            }
        }
    }

    fn on_event(&mut self, event: &x11rb::protocol::Event) {
        use x11rb::protocol::Event;
        use x11rb::protocol::shape::SK;
        self.costs.notifies = self.costs.notifies.saturating_add(1);
        let Event::ShapeNotify(notify) = event else {
            return;
        };
        if notify.shape_kind == SK::INPUT {
            self.costs.input_notifies = self.costs.input_notifies.saturating_add(1);
            println!(
                "ShapeNotify kind=Input window=0x{:x} shaped={} extents={}x{}+{}+{} t_ms={:.0}",
                notify.affected_window,
                notify.shaped,
                notify.extents_width,
                notify.extents_height,
                notify.extents_x,
                notify.extents_y,
                self.started.elapsed().as_secs_f64() * 1000.0
            );
        } else {
            println!(
                "ShapeNotify kind={:?} window=0x{:x} shaped={} extents={}x{}+{}+{}",
                notify.shape_kind,
                notify.affected_window,
                notify.shaped,
                notify.extents_width,
                notify.extents_height,
                notify.extents_x,
                notify.extents_y
            );
        }
    }

    pub fn detect_and_reapply_input(&mut self, enable: bool) -> bool {
        if self.requested_input.is_empty() && self.costs.input_sets == 0 {
            return false;
        }
        let current = self.read_kind(self.window, x11rb::protocol::shape::SK::INPUT);
        let matches = crate::region::rects_match(&current, &self.requested_input, 2.0);
        if matches {
            self.input_matches = true;
            return false;
        }
        if self.input_matches {
            self.costs.wm_resets = self.costs.wm_resets.saturating_add(1);
            let gap_ms = self
                .last_reset
                .map_or(0.0, |prev| prev.elapsed().as_secs_f64() * 1000.0);
            self.last_reset = Some(Instant::now());
            println!(
                "WM_INPUT_RESET n={} gap_ms={gap_ms:.0} now={} requested={}",
                self.costs.wm_resets,
                fmt_rects(&current),
                fmt_rects(&self.requested_input)
            );
        }
        self.input_matches = false;
        if !enable {
            return false;
        }
        let requested = self.requested_input.clone();
        let dt = self.set_kind(x11rb::protocol::shape::SK::INPUT, &requested);
        self.costs.reapplies = self.costs.reapplies.saturating_add(1);
        self.costs.input_ns += dt.as_nanos();
        self.costs.input_sets = self.costs.input_sets.saturating_add(1);
        println!(
            "REAPPLY_INPUT n={} set_us={}",
            self.costs.reapplies,
            dt.as_micros()
        );
        true
    }

    #[must_use]
    pub const fn client(&self) -> u32 {
        self.window
    }

    #[must_use]
    pub const fn frame(&self) -> Option<u32> {
        self.frame
    }
}

#[cfg(target_os = "linux")]
#[must_use]
pub fn fmt_rects(rects: &[ScreenRect]) -> String {
    if rects.is_empty() {
        return "(empty)".to_owned();
    }
    rects
        .iter()
        .map(|rect| {
            format!(
                "{}x{}{:+}{:+}",
                rect.w.round(),
                rect.h.round(),
                rect.x.round(),
                rect.y.round()
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}
