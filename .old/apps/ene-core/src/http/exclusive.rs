use std::sync::atomic::{AtomicBool, Ordering};

use ene_api::{ExclusiveSnapshot, ResourceKind};
use parking_lot::Mutex;

use super::error::{ApiReject, conflict};

/// First-writer exclusive resources (mic, speaker, OS-notify owner).
#[derive(Default)]
pub struct ExclusiveHub {
    mic: Mutex<Option<String>>,
    speaker: Mutex<Option<String>>,
    notify: Mutex<Option<String>>,
    last_used: AtomicBool,
}

impl ExclusiveHub {
    #[must_use]
    pub fn new(last_used: bool) -> Self {
        Self {
            last_used: AtomicBool::new(last_used),
            ..Self::default()
        }
    }

    pub fn set_last_used(&self, last_used: bool) {
        self.last_used.store(last_used, Ordering::SeqCst);
    }

    #[must_use]
    pub fn snapshot(&self) -> ExclusiveSnapshot {
        ExclusiveSnapshot {
            mic: self.mic.lock().clone(),
            speaker: self.speaker.lock().clone(),
            notify: self.notify.lock().clone(),
        }
    }

    pub fn claim(
        &self,
        kind: ResourceKind,
        client_id: &str,
    ) -> Result<ExclusiveSnapshot, ApiReject> {
        let mut slot = match kind {
            ResourceKind::Mic => self.mic.lock(),
            ResourceKind::Speaker => self.speaker.lock(),
            ResourceKind::Notify => self.notify.lock(),
        };
        if let Some(owner) = slot.as_ref()
            && owner != client_id
            && !self.last_used.load(Ordering::SeqCst)
        {
            return Err(conflict(
                "resource_busy",
                "exclusive resource is held by another client",
            ));
        }
        *slot = Some(client_id.to_owned());
        drop(slot);
        Ok(self.snapshot())
    }

    #[must_use]
    pub fn is_holder(&self, kind: ResourceKind, client_id: &str) -> bool {
        self.holder(kind).as_deref() == Some(client_id)
    }

    #[must_use]
    fn holder(&self, kind: ResourceKind) -> Option<String> {
        match kind {
            ResourceKind::Mic => self.mic.lock().clone(),
            ResourceKind::Speaker => self.speaker.lock().clone(),
            ResourceKind::Notify => self.notify.lock().clone(),
        }
    }

    pub fn release(&self, kind: ResourceKind, client_id: &str) -> ExclusiveSnapshot {
        let mut slot = match kind {
            ResourceKind::Mic => self.mic.lock(),
            ResourceKind::Speaker => self.speaker.lock(),
            ResourceKind::Notify => self.notify.lock(),
        };
        if slot.as_deref() == Some(client_id) {
            *slot = None;
        }
        drop(slot);
        self.snapshot()
    }

    pub fn release_client(&self, client_id: &str) {
        for kind in [
            ResourceKind::Mic,
            ResourceKind::Speaker,
            ResourceKind::Notify,
        ] {
            drop(self.release(kind, client_id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_used_patch_allows_steal() {
        let hub = ExclusiveHub::new(false);
        hub.claim(ResourceKind::Mic, "stage").expect("first claim");
        assert!(hub.claim(ResourceKind::Mic, "other").is_err());
        hub.set_last_used(true);
        let snap = hub.claim(ResourceKind::Mic, "other").expect("steal");
        assert_eq!(snap.mic.as_deref(), Some("other"));
    }
}
