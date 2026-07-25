use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    pub multiplier: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
            multiplier: 2.0,
        }
    }
}

impl RetryPolicy {
    pub fn new(
        max_retries: u32,
        initial_backoff: Duration,
        max_backoff: Duration,
        multiplier: f64,
    ) -> Self {
        assert!(
            multiplier.is_finite() && multiplier > 0.0,
            "multiplier must be a finite positive number"
        );
        Self {
            max_retries,
            initial_backoff,
            max_backoff,
            multiplier,
        }
    }
    pub fn backoff_for_attempt(&self, attempt: u32) -> Duration {
        let calculated = self.initial_backoff.as_secs_f64() * self.multiplier.powi(attempt as i32);
        let capped = calculated.min(self.max_backoff.as_secs_f64());
        Duration::from_secs_f64(capped)
    }
    pub async fn retry<F, Fut, T, E>(
        &self,
        operation: F,
        should_retry: fn(&E) -> bool,
    ) -> Result<T, E>
    where
        F: Fn(u32) -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
    {
        let mut last_error: Option<E> = None;
        for attempt in 0..=self.max_retries {
            match operation(attempt).await {
                Ok(value) => return Ok(value),
                Err(e) => {
                    if attempt < self.max_retries && should_retry(&e) {
                        let backoff = self.backoff_for_attempt(attempt);
                        warn!(
                            attempt,
                            max_retries = self.max_retries,
                            backoff_ms = backoff.as_millis(),
                            "retrying operation after error"
                        );
                        tokio::time::sleep(backoff).await;
                        last_error = Some(e);
                    } else {
                        return Err(e);
                    }
                }
            }
        }
        Err(last_error.expect("retry loop exhausted without a last error"))
    }
}

#[derive(Debug)]
pub struct RateLimiter {
    state: Arc<Mutex<RateLimiterState>>,
}
#[derive(Debug)]
struct RateLimiterState {
    capacity: f64,
    tokens: f64,
    rate: f64,
    last_refill: Instant,
}

impl RateLimiter {
    pub fn new(capacity: f64, rate: f64) -> Self {
        assert!(capacity > 0.0, "capacity must be positive");
        assert!(rate > 0.0, "rate must be positive");
        Self {
            state: Arc::new(Mutex::new(RateLimiterState {
                capacity,
                tokens: capacity,
                rate,
                last_refill: Instant::now(),
            })),
        }
    }
    pub fn capacity(&self) -> f64 {
        self.state.lock().capacity
    }
    pub fn rate(&self) -> f64 {
        self.state.lock().rate
    }
    pub fn check(&self) -> Duration {
        let mut state = self.state.lock();
        let now = Instant::now();
        let elapsed = now.duration_since(state.last_refill).as_secs_f64();
        state.tokens = (state.tokens + elapsed * state.rate).min(state.capacity);
        state.last_refill = now;
        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            Duration::ZERO
        } else {
            let needed = 1.0 - state.tokens;
            Duration::from_secs_f64(needed / state.rate)
        }
    }
    pub async fn acquire(&self) {
        let wait = self.check();
        if wait > Duration::ZERO {
            tokio::time::sleep(wait).await;
        }
    }
    pub fn reset(&self) {
        let mut state = self.state.lock();
        state.tokens = state.capacity;
        state.last_refill = Instant::now();
    }
}

impl Clone for RateLimiter {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeoutPolicy {
    pub request_timeout: Duration,
    pub connect_timeout: Duration,
}

impl Default for TimeoutPolicy {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
        }
    }
}
impl TimeoutPolicy {
    pub fn new(request_timeout: Duration, connect_timeout: Duration) -> Self {
        Self {
            request_timeout,
            connect_timeout,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PaginationCursor {
    Cursor(String),
    Offset(u64),
    End,
}
impl PaginationCursor {
    pub fn is_end(&self) -> bool {
        matches!(self, Self::End)
    }
    pub fn as_cursor(&self) -> Option<&str> {
        match self {
            Self::Cursor(c) => Some(c.as_str()),
            _ => None,
        }
    }
    pub fn as_offset(&self) -> Option<u64> {
        match self {
            Self::Offset(n) => Some(*n),
            _ => None,
        }
    }
}

pub struct RequestTimeout {
    deadline: Instant,
}
impl RequestTimeout {
    pub fn new(policy: &TimeoutPolicy) -> Self {
        let deadline = Instant::now() + policy.request_timeout;
        Self { deadline }
    }
    pub async fn run<F, T>(&self, future: F) -> Result<T, crate::error::ConnectorError>
    where
        F: std::future::Future<Output = T>,
    {
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .unwrap_or(Duration::ZERO);
        tokio::time::timeout(remaining, future)
            .await
            .map_err(|_| crate::error::ConnectorError::timeout("request exceeded timeout"))
    }
}

#[derive(Debug)]
pub struct RateLimitCounter {
    remaining: AtomicU64,
    limit: u64,
}
impl RateLimitCounter {
    pub fn new(limit: u64) -> Self {
        Self {
            remaining: AtomicU64::new(limit),
            limit,
        }
    }
    pub fn decrement(&self) {
        self.remaining.fetch_sub(1, Ordering::Relaxed);
    }
    pub fn remaining(&self) -> u64 {
        self.remaining.load(Ordering::Relaxed)
    }
    pub fn is_exhausted(&self) -> bool {
        self.remaining() == 0
    }
    pub fn reset(&self) {
        self.remaining.store(self.limit, Ordering::Relaxed);
    }
}
