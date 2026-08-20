use ene_session::{DisplayDepth, TurnId};
use tokio::sync::broadcast;

/// Live-bus event. Server-side depth decides who receives it (I-38).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveEvent {
    TextDelta {
        turn_id: TurnId,
        text: String,
    },
    InnerMessage {
        turn_id: Option<TurnId>,
        text: String,
    },
    ThinkingDelta {
        turn_id: TurnId,
        text: String,
    },
    ToolSummary {
        turn_id: TurnId,
        line: String,
    },
    ToolDetail {
        turn_id: TurnId,
        name: String,
        args: String,
    },
    TurnEnded {
        turn_id: TurnId,
        outcome: String,
    },
}

#[derive(Debug, Clone)]
struct Envelope {
    min_depth: DisplayDepth,
    event: LiveEvent,
}

/// Broadcast live events with server-side depth filtering.
#[derive(Clone)]
pub struct LiveBus {
    tx: broadcast::Sender<Envelope>,
}

impl LiveBus {
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { tx }
    }

    pub fn emit(&self, min_depth: DisplayDepth, event: LiveEvent) {
        drop(self.tx.send(Envelope { min_depth, event }));
    }

    #[must_use]
    pub fn subscribe(&self, depth: DisplayDepth) -> LiveSubscription {
        LiveSubscription {
            depth,
            rx: self.tx.subscribe(),
        }
    }
}

impl Default for LiveBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Filtered receiver for one display plane.
pub struct LiveSubscription {
    depth: DisplayDepth,
    rx: broadcast::Receiver<Envelope>,
}

impl LiveSubscription {
    /// Next event allowed at this subscription's depth.
    pub async fn recv(&mut self) -> Option<LiveEvent> {
        loop {
            match self.rx.recv().await {
                Ok(envelope) if allowed(self.depth, envelope.min_depth) => {
                    return Some(envelope.event);
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Drain already-queued events (tests).
    pub fn try_drain(&mut self) -> Vec<LiveEvent> {
        let mut out = Vec::new();
        while let Ok(envelope) = self.rx.try_recv() {
            if allowed(self.depth, envelope.min_depth) {
                out.push(envelope.event);
            }
        }
        out
    }
}

const fn allowed(sub: DisplayDepth, min: DisplayDepth) -> bool {
    match (sub, min) {
        (DisplayDepth::Detail, _) | (DisplayDepth::Surface, DisplayDepth::Surface) => true,
        (DisplayDepth::Surface, DisplayDepth::Detail) => false,
    }
}
