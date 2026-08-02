//! Client-side retry / rate-limit / timeout policies for plugin authors.
//!
//! These are the plugin-facing counterparts to what the host's own HTTP layer
//! applies, exposed so a plugin controls its own external-service calls
//! (`ctx.credentials().http_client_with(...)`). They are **unrelated** to the
//! host-side `ene_ai::RetryPolicy`: that type backs the provider retry loop
//! (rand-based full jitter, `run()` helper), while these use a validated
//! constructor plus pure exponential backoff.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ene_plugin_proto::PluginError;
use parking_lot::Mutex;
use tracing::warn;

/// Exponential-backoff retry policy.
///
/// `retry()` runs the operation `max_retries + 1` times at most: attempts
/// `0..max_retries` may sleep-and-retry on a retryable error, and the final
/// attempt always returns its outcome.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryPolicy {
    /// Maximum number of retries after the first attempt.
    pub max_retries: u32,
    /// Backoff before the first retry.
    pub initial_backoff: Duration,
    /// Upper bound on the backoff.
    pub max_backoff: Duration,
    /// Per-retry backoff multiplier (validated finite and positive).
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
    /// Creates a retry policy, validating the backoff multiplier.
    ///
    /// # Errors
    /// Returns [`PluginError::Provider`] when `multiplier` is not a finite
    /// positive number.
    pub fn new(
        max_retries: u32,
        initial_backoff: Duration,
        max_backoff: Duration,
        multiplier: f64,
    ) -> Result<Self, PluginError> {
        if multiplier.is_finite() && multiplier > 0.0 {
            Ok(Self {
                max_retries,
                initial_backoff,
                max_backoff,
                multiplier,
            })
        } else {
            Err(PluginError::provider(
                "multiplier must be a finite positive number",
            ))
        }
    }

    /// Backoff to sleep before the retry after attempt `attempt`.
    #[must_use]
    pub fn backoff_for_attempt(&self, attempt: u32) -> Duration {
        let calculated = self.initial_backoff.as_secs_f64() * self.multiplier.powi(attempt as i32);
        let capped = calculated.min(self.max_backoff.as_secs_f64());
        Duration::from_secs_f64(capped)
    }

    /// Runs `operation` with retry, sleeping the backoff between attempts.
    ///
    /// Attempts `0..max_retries` may sleep-and-retry on a retryable error;
    /// the final attempt (`max_retries`) always returns its outcome, so the
    /// loop has no fall-through and no panic path.
    pub async fn retry<F, Fut, T, E>(
        &self,
        operation: F,
        should_retry: fn(&E) -> bool,
    ) -> Result<T, E>
    where
        F: Fn(u32) -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
    {
        for attempt in 0..self.max_retries {
            match operation(attempt).await {
                Ok(value) => return Ok(value),
                Err(e) => {
                    if should_retry(&e) {
                        let backoff = self.backoff_for_attempt(attempt);
                        warn!(
                            attempt,
                            max_retries = self.max_retries,
                            backoff_ms = backoff.as_millis(),
                            "retrying operation after error"
                        );
                        tokio::time::sleep(backoff).await;
                    } else {
                        return Err(e);
                    }
                }
            }
        }
        operation(self.max_retries).await
    }
}

/// Token-bucket rate limiter.
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
    /// Creates a token-bucket rate limiter.
    ///
    /// # Errors
    /// Returns [`PluginError::Provider`] when `capacity` or `rate` is not
    /// positive.
    pub fn new(capacity: f64, rate: f64) -> Result<Self, PluginError> {
        if capacity > 0.0 && rate > 0.0 {
            Ok(Self {
                state: Arc::new(Mutex::new(RateLimiterState {
                    capacity,
                    tokens: capacity,
                    rate,
                    last_refill: Instant::now(),
                })),
            })
        } else {
            Err(PluginError::provider("capacity and rate must be positive"))
        }
    }

    /// Bucket capacity.
    #[must_use]
    pub fn capacity(&self) -> f64 {
        self.state.lock().capacity
    }

    /// Refill rate (tokens per second).
    #[must_use]
    pub fn rate(&self) -> f64 {
        self.state.lock().rate
    }

    /// Consumes one token, returning how long the caller must wait before the
    /// next token is available.
    #[must_use]
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

    /// Waits until a token is available, then consumes it.
    pub async fn acquire(&self) {
        let wait = self.check();
        if wait > Duration::ZERO {
            tokio::time::sleep(wait).await;
        }
    }

    /// Refills the bucket to capacity.
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

/// Connection / per-request timeouts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeoutPolicy {
    /// Total per-request timeout.
    pub request_timeout: Duration,
    /// TCP connect timeout.
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
    /// Creates a timeout policy.
    #[must_use]
    pub fn new(request_timeout: Duration, connect_timeout: Duration) -> Self {
        Self {
            request_timeout,
            connect_timeout,
        }
    }
}

/// A pagination cursor (opaque string cursor, numeric offset, or end).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PaginationCursor {
    /// Opaque server cursor.
    Cursor(String),
    /// Numeric page offset.
    Offset(u64),
    /// No further pages.
    End,
}

impl PaginationCursor {
    /// Whether the cursor is at the end of the pagination.
    #[must_use]
    pub fn is_end(&self) -> bool {
        matches!(self, Self::End)
    }

    /// The opaque cursor, if this is a [`Cursor`](Self::Cursor).
    #[must_use]
    pub fn as_cursor(&self) -> Option<&str> {
        match self {
            Self::Cursor(c) => Some(c.as_str()),
            _ => None,
        }
    }

    /// The numeric offset, if this is an [`Offset`](Self::Offset).
    #[must_use]
    pub fn as_offset(&self) -> Option<u64> {
        match self {
            Self::Offset(n) => Some(*n),
            _ => None,
        }
    }
}

/// A request deadline derived from a [`TimeoutPolicy`].
#[derive(Debug)]
pub struct RequestTimeout {
    deadline: Instant,
}

impl RequestTimeout {
    /// Creates a deadline `policy.request_timeout` from now.
    #[must_use]
    pub fn new(policy: &TimeoutPolicy) -> Self {
        let deadline = Instant::now() + policy.request_timeout;
        Self { deadline }
    }

    /// Runs `future`, failing with [`PluginError::Timeout`] past the deadline.
    pub async fn run<F, T>(&self, future: F) -> Result<T, PluginError>
    where
        F: std::future::Future<Output = T>,
    {
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .unwrap_or(Duration::ZERO);
        tokio::time::timeout(remaining, future)
            .await
            .map_err(|_| PluginError::timeout("request exceeded timeout"))
    }
}

/// A remaining-quota counter that saturates at zero (never underflows).
#[derive(Debug)]
pub struct RateLimitCounter {
    remaining: AtomicU64,
    limit: u64,
}

impl RateLimitCounter {
    /// Creates a counter with `limit` remaining.
    #[must_use]
    pub fn new(limit: u64) -> Self {
        Self {
            remaining: AtomicU64::new(limit),
            limit,
        }
    }

    /// Decrements the remaining counter, saturating at zero.
    ///
    /// Uses a compare-exchange loop so the counter can never underflow to
    /// `u64::MAX` (which would make [`is_exhausted`](Self::is_exhausted)
    /// permanently false).
    #[expect(
        clippy::let_underscore_must_use,
        reason = "Result<u64, u64> is Copy, so `drop()` would itself trip \
                  clippy::dropping_copy_types; `Err` here just means the \
                  counter was already at zero (closure returned `None`), \
                  which is exactly the saturating no-op this method wants"
    )]
    pub fn decrement(&self) {
        let _ = self
            .remaining
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                (current > 0).then_some(current.saturating_sub(1))
            });
    }

    /// Remaining quota.
    #[must_use]
    pub fn remaining(&self) -> u64 {
        self.remaining.load(Ordering::Relaxed)
    }

    /// Whether the quota is exhausted.
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.remaining() == 0
    }

    /// Restores the quota to the initial limit.
    pub fn reset(&self) {
        self.remaining.store(self.limit, Ordering::Relaxed);
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "tests use expect for concise assertions"
)]
mod tests {
    use super::*;

    #[test]
    fn retry_policy_validates_multiplier() {
        assert!(
            RetryPolicy::new(3, Duration::from_millis(100), Duration::from_secs(10), 2.0).is_ok()
        );
        assert!(
            RetryPolicy::new(3, Duration::from_millis(100), Duration::from_secs(10), 0.0).is_err()
        );
        assert!(
            RetryPolicy::new(
                3,
                Duration::from_millis(100),
                Duration::from_secs(10),
                f64::NAN
            )
            .is_err()
        );
        assert!(
            RetryPolicy::new(
                3,
                Duration::from_millis(100),
                Duration::from_secs(10),
                f64::INFINITY
            )
            .is_err()
        );
    }

    #[test]
    fn backoff_grows_exponentially_and_caps() {
        let policy = RetryPolicy::new(10, Duration::from_millis(100), Duration::from_secs(2), 2.0)
            .expect("valid policy");
        assert_eq!(policy.backoff_for_attempt(0), Duration::from_millis(100));
        assert_eq!(policy.backoff_for_attempt(1), Duration::from_millis(200));
        assert_eq!(policy.backoff_for_attempt(2), Duration::from_millis(400));
        // Capped at max_backoff.
        assert_eq!(policy.backoff_for_attempt(10), Duration::from_secs(2));
    }

    #[tokio::test]
    async fn retry_succeeds_on_first_attempt() {
        let policy = RetryPolicy::default();
        let attempts = std::sync::atomic::AtomicU32::new(0);
        let result = policy
            .retry(
                |_| {
                    attempts.fetch_add(1, Ordering::Relaxed);
                    async { Ok::<u32, u32>(7) }
                },
                |_: &u32| true,
            )
            .await;
        assert_eq!(result, Ok(7));
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn retry_exhausts_max_retries_plus_one() {
        let policy = RetryPolicy::new(3, Duration::from_millis(1), Duration::from_millis(2), 2.0)
            .expect("valid policy");
        let attempts = std::sync::atomic::AtomicU32::new(0);
        let result = policy
            .retry(
                |_| {
                    attempts.fetch_add(1, Ordering::Relaxed);
                    async { Err::<u32, u32>(1) }
                },
                |_: &u32| true,
            )
            .await;
        assert_eq!(result, Err(1));
        // max_retries retries + the initial attempt = 4 total.
        assert_eq!(attempts.load(Ordering::Relaxed), 4);
    }

    #[tokio::test]
    async fn retry_stops_on_non_retryable_error() {
        let policy = RetryPolicy::new(5, Duration::from_millis(1), Duration::from_millis(2), 2.0)
            .expect("valid policy");
        let attempts = std::sync::atomic::AtomicU32::new(0);
        let result = policy
            .retry(
                |_| {
                    attempts.fetch_add(1, Ordering::Relaxed);
                    async { Err::<u32, u32>(1) }
                },
                |_: &u32| false,
            )
            .await;
        assert_eq!(result, Err(1));
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn rate_limiter_rejects_non_positive_parameters() {
        assert!(RateLimiter::new(0.0, 1.0).is_err());
        assert!(RateLimiter::new(1.0, 0.0).is_err());
        assert!(RateLimiter::new(-1.0, 1.0).is_err());
        assert!(RateLimiter::new(10.0, 5.0).is_ok());
    }

    #[test]
    fn rate_limiter_throttles_when_depleted() {
        let limiter = RateLimiter::new(1.0, 10.0).expect("valid limiter");
        // First token is free.
        assert_eq!(limiter.check(), Duration::ZERO);
        // Second call within the same instant must wait.
        assert!(limiter.check() > Duration::ZERO);
    }

    #[test]
    fn rate_limit_counter_never_underflows() {
        let counter = RateLimitCounter::new(2);
        counter.decrement();
        counter.decrement();
        assert_eq!(counter.remaining(), 0);
        assert!(counter.is_exhausted());
        // Saturates at zero instead of wrapping to u64::MAX.
        counter.decrement();
        assert_eq!(counter.remaining(), 0);
        counter.reset();
        assert_eq!(counter.remaining(), 2);
    }

    #[test]
    fn pagination_cursor_accessors() {
        assert!(PaginationCursor::End.is_end());
        assert_eq!(
            PaginationCursor::Cursor("abc".into()).as_cursor(),
            Some("abc")
        );
        assert_eq!(PaginationCursor::Offset(3).as_offset(), Some(3));
        assert_eq!(PaginationCursor::Cursor("x".into()).as_offset(), None);
    }

    #[test]
    fn request_timeout_returns_ok_within_deadline() {
        let policy = TimeoutPolicy::new(Duration::from_secs(1), Duration::from_secs(1));
        let timeout = RequestTimeout::new(&policy);
        let result = tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(timeout.run(async { 42 }));
        assert_eq!(result, Ok(42));
    }
}
