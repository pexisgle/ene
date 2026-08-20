use crate::event::{EventPayload, InboxCancelReason, InboxClass, InboxSource, LoggedEvent};
use crate::ids::TurnId;

/// Unclaimed inbox row reconstructed from the log (D-5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxItem {
    pub seq: u64,
    pub lane: String,
    pub class: InboxClass,
    pub source: InboxSource,
    pub ref_seq: Option<u64>,
}

/// `turn/start` with no matching `turn/end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTurn {
    pub turn_id: TurnId,
    pub start_seq: u64,
    pub lane: String,
}

/// Inbox entries that have neither `claimed` nor `cancelled`.
#[must_use]
pub fn unclaimed_inbox(events: &[LoggedEvent]) -> Vec<InboxItem> {
    let mut open = Vec::new();
    for event in events {
        match &event.payload {
            EventPayload::InboxEnqueued {
                lane,
                class,
                source,
                ref_seq,
                ..
            } => open.push(InboxItem {
                seq: event.seq,
                lane: lane.clone(),
                class: *class,
                source: *source,
                ref_seq: *ref_seq,
            }),
            EventPayload::InboxClaimed { entry_seq, .. }
            | EventPayload::InboxCancelled { entry_seq, .. } => {
                open.retain(|item| item.seq != *entry_seq);
            }
            _ => {}
        }
    }
    open
}

/// Turns whose `turn/start` has no later `turn/end` for the same id.
#[must_use]
pub fn open_turns(events: &[LoggedEvent]) -> Vec<OpenTurn> {
    let mut open = Vec::new();
    for event in events {
        match &event.payload {
            EventPayload::TurnStart { turn_id, lane, .. } => open.push(OpenTurn {
                turn_id: *turn_id,
                start_seq: event.seq,
                lane: lane.clone(),
            }),
            EventPayload::TurnEnd { turn_id, .. } => {
                open.retain(|item| item.turn_id != *turn_id);
            }
            _ => {}
        }
    }
    open
}

/// Inbox cancels written during D-5 recovery.
#[must_use]
pub fn abandoned_inbox(events: &[LoggedEvent]) -> Vec<u64> {
    events
        .iter()
        .filter_map(|event| match event.payload {
            EventPayload::InboxCancelled {
                entry_seq,
                reason: InboxCancelReason::AbandonedInterrupt,
                ..
            } => Some(entry_seq),
            _ => None,
        })
        .collect()
}
