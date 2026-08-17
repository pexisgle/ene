use ene_api::{ExclusiveSnapshot, ResourceKind};
use parking_lot::Mutex;

use super::error::{ApiReject, conflict};

/// First-writer exclusive resources (mic, speaker, OS-notify owner).
#[derive(Default)]
pub struct ExclusiveHub {
    mic: Mutex<Option<String>>,
    speaker: Mutex<Option<String>>,
    notify: Mutex<Option<String>>,
    last_used: bool,
}

impl ExclusiveHub {
    #[must_use]
    pub fn new(last_used: bool) -> Self {
        Self {
            last_used,
            ..Self::default()
        }
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
            && !self.last_used
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
