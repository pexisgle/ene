//! One-shot delivery slots for first-pairing authentication material.

use std::collections::HashMap;
use std::sync::Mutex;

use ene_api::v1::handshake::PairingProvision;
use ene_api::v1::refs::ConnectionWireId;

struct Slot {
    pending_id: Option<String>,
    sender: tokio::sync::mpsc::Sender<PairingProvision>,
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
                sender,
            },
        );
        Some(receiver)
    }

    pub(crate) fn bind_pending(&self, connection: &ConnectionWireId, pending_id: &str) -> bool {
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
        state.by_pending.insert(pending_id.to_owned(), *connection);
        true
    }

    pub(crate) fn claim(&self, pending_id: &str) -> Option<PairingDeliveryClaim> {
        let mut state = crate::lock_unpoison(&self.inner);
        let connection = state.by_pending.remove(pending_id)?;
        let slot = state.by_connection.get_mut(&connection)?;
        if slot.pending_id.as_deref() != Some(pending_id) {
            return None;
        }
        slot.pending_id = None;
        Some(PairingDeliveryClaim {
            connection,
            sender: slot.sender.clone(),
        })
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
