use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

const FORBIDDEN_ATTR_KEYS: &[&str] = &[
    "prompt",
    "completion",
    "content",
    "text",
    "args",
    "result",
    "file",
    "credential",
    "authorization",
    "api_key",
];

/// One local diagnostic span. Attributes must never carry content (P-517).
#[derive(Debug, Clone)]
pub struct Span {
    pub name: String,
    pub duration: Option<Duration>,
    pub attrs: Vec<(String, String)>,
}

/// In-memory ring of spans.
pub struct SpanRing {
    inner: Mutex<VecDeque<Span>>,
    capacity: usize,
    dropped: Mutex<u64>,
}

impl SpanRing {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity.min(1024))),
            capacity: capacity.max(1),
            dropped: Mutex::new(0),
        }
    }

    pub fn record(&self, name: impl Into<String>, started: Instant, attrs: Vec<(String, String)>) {
        let clean: Vec<(String, String)> = attrs
            .into_iter()
            .filter(|(key, _)| {
                let lower = key.to_ascii_lowercase();
                !FORBIDDEN_ATTR_KEYS
                    .iter()
                    .any(|forbidden| lower.contains(forbidden))
            })
            .collect();
        let span = Span {
            name: name.into(),
            duration: Some(started.elapsed()),
            attrs: clean,
        };
        let mut ring = self.inner.lock();
        if ring.len() >= self.capacity {
            ring.pop_front();
            *self.dropped.lock() += 1;
        }
        ring.push_back(span);
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<Span> {
        self.inner.lock().iter().cloned().collect()
    }

    #[must_use]
    pub fn dropped(&self) -> u64 {
        *self.dropped.lock()
    }
}

/// Cloneable handle to the process-local span ring.
#[derive(Clone)]
pub struct ObserveHandle {
    ring: Arc<SpanRing>,
}

impl ObserveHandle {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            ring: Arc::new(SpanRing::new(capacity)),
        }
    }

    #[must_use]
    pub fn start(&self, name: impl Into<String>) -> SpanGuard {
        SpanGuard {
            ring: Arc::clone(&self.ring),
            name: name.into(),
            started: Instant::now(),
            attrs: Vec::new(),
            ended: false,
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<Span> {
        self.ring.snapshot()
    }

    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.ring.dropped()
    }
}

/// Records a span when ended or dropped.
pub struct SpanGuard {
    ring: Arc<SpanRing>,
    name: String,
    started: Instant,
    attrs: Vec<(String, String)>,
    ended: bool,
}

impl SpanGuard {
    #[must_use]
    pub fn attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attrs.push((key.into(), value.into()));
        self
    }

    pub fn end(mut self) {
        self.flush();
        self.ended = true;
    }

    fn flush(&mut self) {
        if self.ended {
            return;
        }
        self.ended = true;
        self.ring.record(
            self.name.clone(),
            self.started,
            std::mem::take(&mut self.attrs),
        );
    }
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        self.flush();
    }
}

/// True when any recorded attribute looks like content (CI guard).
#[must_use]
pub fn spans_leak_content(spans: &[Span]) -> bool {
    spans.iter().any(|span| {
        span.attrs.iter().any(|(key, value)| {
            let lower = key.to_ascii_lowercase();
            FORBIDDEN_ATTR_KEYS
                .iter()
                .any(|forbidden| lower.contains(forbidden))
                || value.len() > 64 && value.contains("ack:")
        })
    })
}
