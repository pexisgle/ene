use std::collections::HashMap;
use std::sync::Mutex;

use ene_api::v1::handshake::PairingProvision;
use ene_api::v1::refs::{ConnectionWireId, RequestWireId};

struct Slot {
    pending_id: Option<String>,
    request_id: Option<RequestWireId>,
    descriptor: Option<String>,
    sender: tokio::sync::mpsc::Sender<PairingProvision>,
}

pub(crate) enum PendingResend {
    Answer(String),
    Conflicting,
}

#[derive(Default)]
pub(crate) struct PairingDeliveryRegistry {
    inner: Mutex<RegistryState>,
}

#[derive(Default)]
struct RegistryState {
    by_connection: HashMap<ConnectionWireId, Slot>,
    by_pending: HashMap<String, ConnectionWireId>,
}

pub(crate) struct PairingDeliveryClaim {
    pub(crate) connection: ConnectionWireId,
    pending_id: String,
    request_id: Option<RequestWireId>,
    descriptor: String,
    sender: tokio::sync::mpsc::Sender<PairingProvision>,
}

impl PairingDeliveryClaim {
    pub(crate) fn queue(self, provision: PairingProvision) -> Result<(), PairingProvision> {
        self.sender
            .try_send(provision)
            .map_err(tokio::sync::mpsc::error::TrySendError::into_inner)
    }
}

impl PairingDeliveryRegistry {
    pub(crate) fn register(
        &self,
        connection: &ConnectionWireId,
    ) -> Option<tokio::sync::mpsc::Receiver<PairingProvision>> {
        let mut state = crate::lock_unpoison(&self.inner);
        if state.by_connection.contains_key(connection) {
            return None;
        }
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        state.by_connection.insert(
            *connection,
            Slot {
                pending_id: None,
                request_id: None,
                descriptor: None,
                sender,
            },
        );
        Some(receiver)
    }

    pub(crate) fn bind_pending(
        &self,
        connection: &ConnectionWireId,
        pending_id: &str,
        request_id: Option<RequestWireId>,
        descriptor: &str,
    ) -> bool {
        let mut state = crate::lock_unpoison(&self.inner);
        if state.by_pending.contains_key(pending_id) {
            return false;
        }
        let Some(slot) = state.by_connection.get_mut(connection) else {
            return false;
        };
        if slot.pending_id.is_some() {
            return false;
        }
        slot.pending_id = Some(pending_id.to_owned());
        slot.request_id = request_id;
        slot.descriptor = Some(descriptor.to_owned());
        state.by_pending.insert(pending_id.to_owned(), *connection);
        true
    }

    pub(crate) fn resend_match(
        &self,
        connection: &ConnectionWireId,
        request_id: Option<RequestWireId>,
        descriptor: &str,
    ) -> Option<PendingResend> {
        let state = crate::lock_unpoison(&self.inner);
        let slot = state.by_connection.get(connection)?;
        let pending_id = slot.pending_id.clone()?;
        if slot.request_id == request_id && slot.descriptor.as_deref() != Some(descriptor) {
            return Some(PendingResend::Conflicting);
        }
        Some(PendingResend::Answer(pending_id))
    }

    pub(crate) fn claim(&self, pending_id: &str) -> Option<PairingDeliveryClaim> {
        let mut state = crate::lock_unpoison(&self.inner);
        let connection = state.by_pending.remove(pending_id)?;
        let slot = state.by_connection.get_mut(&connection)?;
        if slot.pending_id.as_deref() != Some(pending_id) {
            return None;
        }
        let request_id = slot.request_id.take();
        let descriptor = slot.descriptor.take().unwrap_or_default();
        slot.pending_id = None;
        Some(PairingDeliveryClaim {
            connection,
            pending_id: pending_id.to_string(),
            request_id,
            descriptor,
            sender: slot.sender.clone(),
        })
    }

    pub(crate) fn release(&self, claim: PairingDeliveryClaim) {
        let mut state = crate::lock_unpoison(&self.inner);
        if state.by_pending.contains_key(&claim.pending_id) {
            return;
        }
        let Some(slot) = state.by_connection.get_mut(&claim.connection) else {
            return;
        };
        if slot.pending_id.is_some() {
            return;
        }
        slot.pending_id = Some(claim.pending_id.clone());
        slot.request_id = claim.request_id;
        slot.descriptor = Some(claim.descriptor);
        state.by_pending.insert(claim.pending_id, claim.connection);
    }

    pub(crate) fn remove(&self, connection: &ConnectionWireId) {
        let mut state = crate::lock_unpoison(&self.inner);
        let Some(slot) = state.by_connection.remove(connection) else {
            return;
        };
        if let Some(pending_id) = slot.pending_id {
            state.by_pending.remove(&pending_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PairingDeliveryRegistry;
    use ene_api::v1::refs::ConnectionWireId;
    use uuid::Uuid;

    #[test]
    fn a_bound_pending_is_claimed_once_only_while_its_connection_is_registered() {
        for (case, remove_connection, expected_first_claim) in [
            ("registered", false, true),
            ("connection removed", true, false),
        ] {
            let registry = PairingDeliveryRegistry::default();
            let connection = ConnectionWireId(Uuid::new_v4());
            let receiver = registry.register(&connection);
            assert!(receiver.is_some(), "{case}: registration must succeed");
            assert!(
                registry.bind_pending(&connection, "pending-1", None, "laptop"),
                "{case}: the registered connection must bind its pending"
            );
            if remove_connection {
                registry.remove(&connection);
            }
            assert_eq!(
                registry.claim("pending-1").is_some(),
                expected_first_claim,
                "{case}: connection removal controls claim authority"
            );
            assert!(
                registry.claim("pending-1").is_none(),
                "{case}: a pending is claimed at most once"
            );
        }
    }
}
