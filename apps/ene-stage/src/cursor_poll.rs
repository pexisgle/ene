//! X11 global cursor polling for hover re-arming behind a click-through
//! surface. On X11, an empty input shape stops all pointer events, so the
//! overlay polls the root-window pointer to detect when it moves back over a
//! body and can restore input.
//!
//! Wayland cannot query the global pointer, so this module is X11-only.

use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender};
use tracing::debug;
use x11rb::connection::Connection;

/// Global pointer position in screen coordinates, polled from X11.
#[derive(Debug, Clone, Copy)]
pub struct GlobalCursor {
    pub x: f64,
    pub y: f64,
}

/// Spawns a background thread that polls the X11 root-window pointer at a
/// fixed interval. Returns `None` when the display is unavailable (non-X11,
/// headless CI) so callers only need to handle the absence.
pub fn spawn(poll_interval_ms: u64) -> Option<(JoinHandle<()>, Receiver<GlobalCursor>)> {
    let (conn, screen_num) = match x11rb::connect(None) {
        Ok(pair) => pair,
        Err(err) => {
            debug!("X11 connect failed; cursor polling disabled: {err}");
            return None;
        }
    };
    let root = conn.setup().roots[screen_num].root;

    let (tx, rx): (Sender<GlobalCursor>, Receiver<GlobalCursor>) = crossbeam_channel::bounded(16);
    let handle = thread::Builder::new()
        .name("ene-cursor-poll".to_owned())
        .spawn(move || {
            loop {
                match x11rb::protocol::xproto::query_pointer(&conn, root) {
                    Ok(cookie) => match cookie.reply() {
                        Ok(reply) => {
                            let cursor = GlobalCursor {
                                x: f64::from(reply.root_x),
                                y: f64::from(reply.root_y),
                            };
                            // Bounded channel: drop stale positions when the
                            // consumer is slower than the poll rate.
                            if tx.try_send(cursor).is_err() {
                                // Consumer is slower than the poll rate; the
                                // stale position is dropped on purpose.
                            }
                        }
                        Err(err) => debug!("query_pointer reply failed: {err}"),
                    },
                    Err(err) => {
                        debug!("query_pointer failed; stopping cursor poll: {err}");
                        break;
                    }
                }
                thread::sleep(std::time::Duration::from_millis(poll_interval_ms));
            }
        });
    handle.map_or_else(
        |err| {
            debug!("cursor poll thread failed to spawn: {err}");
            None
        },
        |h| Some((h, rx)),
    )
}
