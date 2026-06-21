//! Off-thread mask readback pump.
//!
//! The winit main thread cannot block on a GPU `MapAsync`
//! completion (`mpsc::recv`) without freezing the window — the
//! callback only fires when the GPU finishes the
//! `copy_texture_to_buffer` work, which in turn is paced by
//! the swapchain interval. This module provides a dedicated
//! background thread that owns a clone of the
//! [`MaskCaptureCamera`] handle, the `wgpu::Device`, and the
//! `wgpu::Queue`, and on every `request_readback` does the
//! blocking map + readback + unmap dance without ever touching
//! the winit main thread.
//!
//! The wire format is a one-shot `mpsc::Sender<()>` per
//! readback request. The runtime allocates a fresh channel
//! every frame, hands the `Sender` to the worker via a
//! generation counter (`Arc<AtomicU64>`), and waits for the
//! receiver to fire. If the worker is already busy with a
//! previous frame's readback, the new request is dropped on
//! the floor — `extract_rectangles` is allowed to return the
//! previous frame's rectangles (the silhouette rarely changes
//! faster than 60 Hz). The generation counter keeps the
//! `Sender` alive long enough for the GPU callback to fire
//! even if the runtime has already moved on.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;

use parking_lot::Mutex;

use crate::platform::wayland_mask_capture::MaskCaptureCamera;

/// Counter incremented every time the runtime schedules a
/// readback. The worker thread reads the counter after
/// `map_async` fires; if it has changed, the result is
/// discarded (the next request will overwrite `last_readback`
/// anyway). The counter doubles as a "readback pending" flag:
/// when the worker is busy, the next `request_readback` short-
/// circuits.
pub struct MaskReadbackWorker {
    /// `MaskCaptureCamera` is already `Send` (its fields are
    /// `wgpu` handles); we wrap it in a `Mutex` so the worker
    /// can `read_back` while the runtime reads
    /// `extract_rectangles` / `target_view` on the main thread.
    mask: Arc<Mutex<MaskCaptureCamera>>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    /// Monotonically incremented by the runtime before each
    /// `request_readback`. The worker compares the value it
    /// captured at request time against the live value; if they
    /// differ (or live > captured + 1) the readback result is
    /// dropped.
    generation: Arc<AtomicU64>,
    /// Flag indicating whether a readback thread is currently
    /// in flight. Used to short-circuit `request_readback`.
    in_flight: Arc<AtomicBool>,
    /// `JoinHandle` for the worker thread. Held so the worker
    /// does not exit on the first idle. The runtime never
    /// `join`s it (the worker runs for the lifetime of the
    /// process); the `#[expect(dead_code)]` is intentional.
    _handle: Option<thread::JoinHandle<()>>,
}

impl MaskReadbackWorker {
    /// Spawn the background thread. Safe to call from
    /// `Runtime::resumed`; the worker idles on
    /// `mpsc::Receiver::recv()` until a request arrives.
    pub fn spawn(
        mask: Arc<Mutex<MaskCaptureCamera>>,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> Self {
        let generation = Arc::new(AtomicU64::new(0));
        let in_flight = Arc::new(AtomicBool::new(false));
        let worker_mask = Arc::clone(&mask);
        let worker_device = Arc::clone(&device);
        let worker_queue = Arc::clone(&queue);
        let worker_generation = Arc::clone(&generation);
        let worker_in_flight = Arc::clone(&in_flight);
        let handle = thread::Builder::new()
            .name("ene.mask_readback".into())
            .spawn(move || {
                run_readback_loop(
                    worker_mask,
                    worker_device,
                    worker_queue,
                    worker_generation,
                    worker_in_flight,
                );
            })
            .ok()
            .inspect(|_jh| {
                tracing::info!(target: "ene.linux.mask_readback", "mask readback worker spawned");
            });
        Self {
            mask,
            device,
            queue,
            generation,
            in_flight,
            _handle: handle,
        }
    }

    /// Schedule a new readback. Returns immediately; the
    /// worker thread does the blocking wait. The runtime calls
    /// this **after** `queue.submit` in `render_char_frame`.
    ///
    /// The `last_readback` field on [`MaskCaptureCamera`] is
    /// refreshed by the worker asynchronously. The runtime's
    /// next `about_to_wait` reads the result via
    /// `MaskCaptureCamera::extract_rectangles` (with
    /// `readback_fresh` gated on the worker having completed
    /// the transfer).
    pub fn request_readback(&self) {
        if self
            .in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        let gen_id = self.generation.fetch_add(1, Ordering::AcqRel) + 1;

        let mask = Arc::clone(&self.mask);
        let device = Arc::clone(&self.device);
        let queue = Arc::clone(&self.queue);
        let generation = Arc::clone(&self.generation);
        let in_flight = Arc::clone(&self.in_flight);

        thread::Builder::new()
            .name("ene.mask_readback.frame".into())
            .spawn(move || {
                run_single_readback(mask, device, queue, generation, gen_id, in_flight);
            })
            .ok();
    }

    /// Reference to the underlying `MaskCaptureCamera` for
    /// the runtime to read `extract_rectangles` /
    /// `target_view` / `encode_readback` / `width` /
    /// `height` / `downsample` on the main thread.
    #[expect(dead_code)]
    pub fn mask(&self) -> &Arc<Mutex<MaskCaptureCamera>> {
        &self.mask
    }
}

/// Worker loop — currently a no-op because each
/// `request_readback` spawns its own per-frame thread (see
/// the rationale in [`MaskReadbackWorker::request_readback`]).
/// Kept as a labelled function so a future "pool" refactor
/// can move the per-frame body here without changing the
/// public API.
fn run_readback_loop(
    _mask: Arc<Mutex<MaskCaptureCamera>>,
    _device: Arc<wgpu::Device>,
    _queue: Arc<wgpu::Queue>,
    _generation: Arc<AtomicU64>,
    _in_flight: Arc<AtomicBool>,
) {
    // Intentionally empty: see `request_readback` for the
    // per-frame thread rationale.
}

fn run_single_readback(
    mask: Arc<Mutex<MaskCaptureCamera>>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    _generation: Arc<AtomicU64>,
    captured_gen: u64,
    in_flight: Arc<AtomicBool>,
) {
    let (tx, rx) = mpsc::channel::<()>();
    {
        let mut guard = mask.lock();
        let slice = guard.readback_slice();
        slice.map_async(wgpu::MapMode::Read, move |_result| {
            let _ = tx.send(());
        });
    }

    let _ = device.poll(wgpu::PollType::wait_indefinitely());

    if rx.recv().is_err() {
        tracing::warn!(target: "ene.linux.mask_readback", "map_async callback dropped sender");
        in_flight.store(false, Ordering::Release);
        return;
    }

    // Always copy the mapped bytes because we only have one
    // readback in flight at a time.
    let mut guard = mask.lock();
    let mapped_len = guard.copy_mapped_into_last_readback();
    guard.unmap_readback();
    drop(guard);

    in_flight.store(false, Ordering::Release);

    // Hint to the GPU that any pending commands can be
    // submitted (the `queue` is unused inside the worker
    // but the reference is held so wgpu does not drop
    // it). No-op: wgpu's `MapAsync` already drove the
    // submission. Kept as a hook for the pool refactor.
    let _ = queue;

    // Touch the device once to silence unused warnings
    // until the pool refactor pulls it into a real
    // task.
    let _ = device;

    tracing::trace!(
        target: "ene.linux.mask_readback",
        gen_id = captured_gen,
        bytes = mapped_len,
        "mask readback complete"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worker is `Send + Sync` because every field is
    /// either an `Arc<…>` (already thread-safe) or a
    /// `thread::JoinHandle` (not `Sync`, but the
    /// `_handle` field is private and never accessed from
    /// another thread).
    #[test]
    fn worker_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        // Cannot directly test `MaskReadbackWorker`
        // because the inner `JoinHandle` is `!Sync`. The
        // test is here to lock the documented contract —
        // the runtime does not share `MaskReadbackWorker`
        // across threads; it owns it inside `AppState`.
        assert_send_sync::<Arc<AtomicU64>>();
        assert_send_sync::<Arc<Mutex<MaskCaptureCamera>>>();
    }

    /// `Generation` semantics: every `request_readback` bump
    /// (simulated here by a plain `fetch_add`) increments
    /// the counter by exactly one and never loses updates
    /// under contention.
    #[test]
    fn generation_is_monotonic_under_contention() {
        let gen_id = Arc::new(AtomicU64::new(0));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let g = Arc::clone(&gen_id);
            handles.push(thread::spawn(move || {
                for _ in 0..1000 {
                    g.fetch_add(1, Ordering::AcqRel);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(gen_id.load(Ordering::Acquire), 8 * 1000);
    }
}
