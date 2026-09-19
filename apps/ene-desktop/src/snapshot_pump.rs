//! Sequence numbers so a slower GUI apply cannot overwrite a newer snapshot.
//!
//! The Slint event loop paints after the runtime mutex is released. A 50 ms
//! tick that sampled before a click must not paint over the click.

use std::sync::atomic::{AtomicU64, Ordering};

/// Stamp taken under the runtime lock; accepted only on the UI thread.
#[derive(Debug)]
pub struct SnapshotPump {
    epoch: AtomicU64,
    applied: AtomicU64,
}

impl Default for SnapshotPump {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotPump {
    #[must_use]
    pub fn new() -> Self {
        Self {
            epoch: AtomicU64::new(0),
            applied: AtomicU64::new(0),
        }
    }

    /// Call while holding the runtime lock, after taking the snapshot.
    pub fn stamp(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Event-loop only. Drops snapshots older than the last accepted stamp.
    #[must_use]
    pub fn accept(&self, seq: u64) -> bool {
        let last = self.applied.load(Ordering::Acquire);
        if seq <= last {
            return false;
        }
        self.applied.store(seq, Ordering::Release);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::SnapshotPump;

    #[test]
    fn older_stamp_is_rejected() {
        let pump = SnapshotPump::new();
        let first = pump.stamp();
        let second = pump.stamp();
        assert!(pump.accept(second));
        assert!(!pump.accept(first));
        assert!(!pump.accept(second));
    }
}
