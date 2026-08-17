use crate::types::CompanionReport;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Holds completion reports while the user is speaking; drains on a gap.
pub struct SpeechGate {
    user_speaking: AtomicBool,
    pending: Mutex<Vec<CompanionReport>>,
}

impl SpeechGate {
    #[must_use]
    pub fn new() -> Self {
        Self {
            user_speaking: AtomicBool::new(false),
            pending: Mutex::new(Vec::new()),
        }
    }

    pub fn set_user_speaking(&self, active: bool) {
        self.user_speaking.store(active, Ordering::SeqCst);
    }

    #[must_use]
    pub fn user_speaking(&self) -> bool {
        self.user_speaking.load(Ordering::SeqCst)
    }

    /// Deliver now, or queue when the user is still talking.
    pub fn offer(&self, report: CompanionReport) -> Option<CompanionReport> {
        if report.starts_conversation && self.user_speaking() {
            self.pending.lock().push(report);
            None
        } else {
            Some(report)
        }
    }

    /// Release queued reports once the user has stopped.
    pub fn drain_when_gap(&self) -> Vec<CompanionReport> {
        if self.user_speaking() {
            return Vec::new();
        }
        let mut pending = self.pending.lock();
        let out = pending.clone();
        pending.clear();
        out
    }
}

impl Default for SpeechGate {
    fn default() -> Self {
        Self::new()
    }
}
