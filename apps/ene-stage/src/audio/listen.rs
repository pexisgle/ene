//! Mic listen WebSocket lifecycle: generation, drop-on-close, bounded retry.

use std::time::{Duration, Instant};

use tokio::sync::mpsc::{self, Receiver, Sender, error::TrySendError};

/// Wait after a finished stream before opening another (mic still claimed).
pub const LISTEN_RETRY: Duration = Duration::from_millis(250);

/// Outcome of [`MicListen::try_send`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendResult {
    Sent,
    Full,
    Closed,
    Idle,
}

/// What the UI should do after a listen-state change.
pub enum ListenAction {
    Spawn {
        generation: u64,
        rx: Receiver<Vec<f32>>,
    },
    None,
}

/// Owned sender + generation for the in-flight listen task.
#[derive(Debug, Default)]
pub struct MicListen {
    tx: Option<Sender<Vec<f32>>>,
    generation: u64,
    next_retry: Option<Instant>,
}

impl MicListen {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Open a stream now (mic just claimed). Ignores retry backoff.
    pub fn start(&mut self) -> ListenAction {
        self.next_retry = None;
        self.open()
    }

    /// Drop the sender and invalidate in-flight outcomes (mic released).
    pub fn release(&mut self) {
        self.tx = None;
        self.generation = self.generation.wrapping_add(1);
        self.next_retry = None;
    }

    /// After the listen task returns: clear if this generation is still current.
    pub fn on_done(&mut self, generation: u64, mic_active: bool, now: Instant) -> ListenAction {
        if generation != self.generation {
            return ListenAction::None;
        }
        self.tx = None;
        if !mic_active {
            self.next_retry = None;
            return ListenAction::None;
        }
        self.next_retry = Some(now + LISTEN_RETRY);
        ListenAction::None
    }

    /// Reconnect when mic is claimed, the sender is gone, and backoff has elapsed.
    pub fn poll(&mut self, mic_active: bool, now: Instant) -> ListenAction {
        if !mic_active {
            self.release();
            return ListenAction::None;
        }
        if self.tx.is_some() {
            return ListenAction::None;
        }
        if let Some(at) = self.next_retry
            && now < at
        {
            return ListenAction::None;
        }
        self.next_retry = None;
        self.open()
    }

    pub fn try_send(&mut self, batch: Vec<f32>) -> SendResult {
        let Some(tx) = self.tx.as_ref() else {
            return SendResult::Idle;
        };
        match tx.try_send(batch) {
            Ok(()) => SendResult::Sent,
            Err(TrySendError::Full(_)) => SendResult::Full,
            Err(TrySendError::Closed(_)) => {
                self.tx = None;
                SendResult::Closed
            }
        }
    }

    fn open(&mut self) -> ListenAction {
        if self.tx.is_some() {
            return ListenAction::None;
        }
        let (tx, rx) = mpsc::channel(8);
        self.generation = self.generation.wrapping_add(1);
        self.tx = Some(tx);
        ListenAction::Spawn {
            generation: self.generation,
            rx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_sender_reconnects_and_next_batch_arrives() {
        let mut listen = MicListen::new();
        let t0 = Instant::now();
        let ListenAction::Spawn {
            generation: first,
            rx: rx1,
        } = listen.start()
        else {
            panic!("expected spawn");
        };
        drop(rx1);
        assert_eq!(listen.try_send(vec![0.1]), SendResult::Closed);
        let ListenAction::Spawn {
            generation: second,
            rx: mut rx2,
        } = listen.poll(true, t0)
        else {
            panic!("expected reconnect");
        };
        assert_ne!(first, second);
        assert_eq!(listen.try_send(vec![0.25]), SendResult::Sent);
        assert_eq!(rx2.try_recv().expect("batch"), vec![0.25]);
    }

    #[test]
    fn listen_done_reconnects_after_backoff_while_mic_claimed() {
        let mut listen = MicListen::new();
        let t0 = Instant::now();
        let ListenAction::Spawn {
            generation: first, ..
        } = listen.start()
        else {
            panic!("expected spawn");
        };
        assert!(matches!(
            listen.on_done(first, true, t0),
            ListenAction::None
        ));
        assert!(matches!(listen.poll(true, t0), ListenAction::None));
        let ListenAction::Spawn {
            generation: second,
            mut rx,
        } = listen.poll(true, t0 + LISTEN_RETRY)
        else {
            panic!("expected spawn after backoff");
        };
        assert_ne!(first, second);
        assert_eq!(listen.try_send(vec![1.0]), SendResult::Sent);
        assert_eq!(rx.try_recv().expect("batch"), vec![1.0]);
    }

    #[test]
    fn stale_generation_does_not_drop_current_stream() {
        let mut listen = MicListen::new();
        let t0 = Instant::now();
        let ListenAction::Spawn {
            generation: first, ..
        } = listen.start()
        else {
            panic!("expected spawn");
        };
        listen.on_done(first, true, t0);
        let ListenAction::Spawn {
            generation: second, ..
        } = listen.poll(true, t0 + LISTEN_RETRY)
        else {
            panic!("expected second stream");
        };
        listen.on_done(first, true, t0 + LISTEN_RETRY);
        assert_eq!(listen.generation(), second);
        assert_eq!(listen.try_send(vec![0.5]), SendResult::Sent);
    }

    #[test]
    fn release_does_not_reconnect() {
        let mut listen = MicListen::new();
        let t0 = Instant::now();
        let ListenAction::Spawn { generation, .. } = listen.start() else {
            panic!("expected spawn");
        };
        listen.release();
        assert!(matches!(
            listen.on_done(generation, false, t0),
            ListenAction::None
        ));
        assert!(matches!(
            listen.poll(false, t0 + LISTEN_RETRY),
            ListenAction::None
        ));
        assert_eq!(listen.try_send(vec![0.0]), SendResult::Idle);
    }

    #[test]
    fn full_keeps_sender() {
        let mut listen = MicListen::new();
        let ListenAction::Spawn { rx, .. } = listen.start() else {
            panic!("expected spawn");
        };
        for _ in 0..8 {
            assert_eq!(listen.try_send(vec![0.0]), SendResult::Sent);
        }
        assert_eq!(listen.try_send(vec![1.0]), SendResult::Full);
        drop(rx);
    }
}
