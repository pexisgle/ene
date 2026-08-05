//! Common transport policies: timeout, backoff retry, rate limiting, pagination.
//!
//! The policies are client-agnostic — connectors pair them with their own
//! HTTP client. The registry applies [`ConnectorPolicy::timeout`] around
//! every lifecycle operation, and connectors use the retry / rate-limit /
//! pagination helpers inside their operation boundary.

use crate::error::ConnectorError;
use std::future::Future;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Per-connector transport policy applied to lifecycle operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorPolicy {
    /// Maximum wall time for one lifecycle operation.
    pub timeout: Duration,
    /// Optional backoff retry policy for transient failures.
    pub retry: Option<RetryPolicy>,
    /// Optional rate limiter shared by the connector's requests.
    pub rate_limit: Option<RateLimitPolicy>,
}

/// Exponential backoff retry policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total attempts including the first (1 = no retry).
    pub max_attempts: u32,
    /// Delay before the first retry.
    pub base_delay: Duration,
    /// Upper bound for any single delay.
    pub max_delay: Duration,
    /// Whether to apply 50%–150% jitter to each delay.
    pub jitter: bool,
}

impl RetryPolicy {
    /// Creates a policy with the given attempt cap and backoff bounds.
    #[must_use]
    pub const fn new(max_attempts: u32, base_delay: Duration, max_delay: Duration) -> Self {
        Self {
            max_attempts: if max_attempts < 1 { 1 } else { max_attempts },
            base_delay,
            max_delay,
            jitter: true,
        }
    }

    /// Disables jitter (deterministic delays; useful in tests).
    #[must_use]
    pub const fn without_jitter(mut self) -> Self {
        self.jitter = false;
        self
    }
}

/// Token-bucket rate-limit policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitPolicy {
    /// Maximum burst of immediate acquisitions.
    pub max_burst: u32,
    /// Time to refill one token.
    pub refill_interval: Duration,
}

impl RateLimitPolicy {
    /// Creates a policy for `max_burst` immediate requests and one token per
    /// `refill_interval`.
    #[must_use]
    pub const fn new(max_burst: u32, refill_interval: Duration) -> Self {
        Self {
            max_burst: if max_burst < 1 { 1 } else { max_burst },
            refill_interval,
        }
    }
}

/// Pagination bounds for cursor-driven collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaginationPolicy {
    /// Maximum number of pages collected per call.
    pub max_pages: u32,
    /// Requested page size; the service may return fewer items.
    pub page_size: u32,
}

impl PaginationPolicy {
    /// Creates a policy with the given page cap and page size.
    #[must_use]
    pub const fn new(max_pages: u32, page_size: u32) -> Self {
        Self {
            max_pages: if max_pages < 1 { 1 } else { max_pages },
            page_size: if page_size < 1 { 1 } else { page_size },
        }
    }
}

/// One page produced by a paginated fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// Items on this page.
    pub items: Vec<T>,
    /// Opaque cursor for the next page, or `None` when this was the last.
    pub next_page_token: Option<String>,
}

/// Computes the delay before retry `attempt` (1-based), capped and optionally
/// jittered.
///
/// `jitter_seed` drives a deterministic xorshift PRNG so callers can pin
/// exact delays in tests; jitter exists to avoid thundering-herd retries and
/// is not security-sensitive.
#[must_use]
pub fn backoff_delay(policy: &RetryPolicy, attempt: u32, jitter_seed: u64) -> Duration {
    let exponent = attempt.saturating_sub(1).min(30);
    let doubled = policy.base_delay.saturating_mul(1_u32 << exponent);
    let capped = doubled.min(policy.max_delay);
    if !policy.jitter {
        return capped;
    }
    let fraction = (0.5 + 0.5 * deterministic_unit(jitter_seed)).clamp(0.5, 1.5);
    Duration::from_secs_f64(capped.as_secs_f64() * fraction)
}

/// Deterministic unit value in `[0, 1)` from a xorshift64 seed.
fn deterministic_unit(seed: u64) -> f64 {
    let mut state = seed.max(1);
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    (state >> 11) as f64 / (1_u64 << 53) as f64
}

/// Runs `op` with exponential backoff, retrying only
/// [`ConnectorError::is_retryable`] failures.
///
/// The whole retry sequence is bounded by `policy.max_attempts`; callers
/// wrap the sequence in their operation timeout so a stuck service cannot
/// run past the operation boundary.
///
/// # Errors
/// Returns the last error once attempts are exhausted, or the first
/// non-retryable error.
pub async fn retry_with_backoff<F, Fut>(
    policy: &RetryPolicy,
    mut op: F,
) -> Result<(), ConnectorError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), ConnectorError>>,
{
    let mut last_error = None;
    for attempt in 1..=policy.max_attempts {
        match op().await {
            Ok(()) => return Ok(()),
            Err(err) if err.is_retryable() && attempt < policy.max_attempts => {
                let seed = now_seed() ^ u64::from(attempt);
                tokio::time::sleep(backoff_delay(policy, attempt, seed)).await;
                last_error = Some(err);
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_error.unwrap_or_else(|| ConnectorError::internal("retry loop exhausted")))
}

/// Returns a per-call seed that differs between attempts but stays
/// deterministic within one call.
fn now_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    u64::from(nanos) ^ 0x9E37_79B9_7F4A_7C15
}

/// Collects every page from a cursor-driven paginated endpoint.
///
/// `fetch` receives the opaque next-page token (`None` for the first page)
/// and returns the page plus the following token. Iteration stops when a
/// page carries no next token or `policy.max_pages` is reached.
///
/// # Errors
/// Returns the first page error; pages already collected are dropped.
pub async fn collect_pages<T, E, F, Fut>(
    policy: &PaginationPolicy,
    mut fetch: F,
) -> Result<Vec<T>, E>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<Page<T>, E>>,
{
    let mut items = Vec::new();
    let mut next_token = None;
    for _ in 0..policy.max_pages {
        let page = fetch(next_token).await?;
        next_token = page.next_page_token;
        items.extend(page.items);
        if next_token.is_none() {
            break;
        }
    }
    Ok(items)
}

/// Async token-bucket rate limiter.
///
/// [`acquire`](Self::acquire) waits until a token is available; the wait is
/// interruptible only by the caller's timeout wrapper, so a burst never
/// exceeds `max_burst` and refills happen at `refill_interval` per token.
pub struct RateLimiter {
    policy: RateLimitPolicy,
    state: Mutex<RateLimiterState>,
}

struct RateLimiterState {
    tokens: f64,
    last_refill: Instant,
}

impl RateLimiter {
    /// Creates a bucket starting full, so up to `max_burst` acquisitions
    /// pass immediately before the refill rate applies.
    #[must_use]
    pub fn new(policy: RateLimitPolicy) -> Self {
        Self {
            policy,
            state: Mutex::new(RateLimiterState {
                tokens: f64::from(policy.max_burst),
                last_refill: Instant::now(),
            }),
        }
    }

    /// Waits until a token is available, then consumes it.
    pub async fn acquire(&self) {
        let mut state = self.state.lock().await;
        loop {
            let now = Instant::now();
            let elapsed = now.duration_since(state.last_refill);
            let refilled = elapsed.as_secs_f64() / self.policy.refill_interval.as_secs_f64();
            state.tokens = (state.tokens + refilled).min(f64::from(self.policy.max_burst));
            state.last_refill = now;
            if state.tokens >= 1.0 {
                state.tokens -= 1.0;
                return;
            }
            let missing = self.policy.refill_interval.as_secs_f64() * (1.0 - state.tokens);
            drop(state);
            tokio::time::sleep(Duration::from_secs_f64(missing)).await;
            state = self.state.lock().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn backoff_doubles_and_caps() {
        let policy = RetryPolicy::new(4, Duration::from_millis(100), Duration::from_secs(1))
            .without_jitter();
        assert_eq!(backoff_delay(&policy, 1, 0), Duration::from_millis(100));
        assert_eq!(backoff_delay(&policy, 2, 0), Duration::from_millis(200));
        assert_eq!(backoff_delay(&policy, 3, 0), Duration::from_millis(400));
        assert_eq!(backoff_delay(&policy, 4, 0), Duration::from_millis(800));
        // Exponent beyond the cap stays at the cap.
        assert_eq!(backoff_delay(&policy, 9, 0), Duration::from_secs(1));
    }

    #[test]
    fn jitter_stays_within_half_and_half_again() {
        let policy = RetryPolicy::new(4, Duration::from_secs(1), Duration::from_secs(8));
        for seed in 1..=64 {
            let delay = backoff_delay(&policy, 2, seed);
            assert!(delay >= Duration::from_millis(500));
            assert!(delay <= Duration::from_millis(1500));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn retry_stops_at_first_success() {
        let attempts = AtomicU32::new(0);
        let policy = RetryPolicy::new(5, Duration::from_millis(10), Duration::from_millis(10))
            .without_jitter();
        retry_with_backoff(&policy, || {
            let attempts = &attempts;
            async move {
                let n = attempts.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(ConnectorError::transport("flaky"))
                } else {
                    Ok(())
                }
            }
        })
        .await
        .expect("third attempt succeeds");
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_gives_up_after_max_attempts() {
        let attempts = AtomicU32::new(0);
        let policy = RetryPolicy::new(3, Duration::from_millis(10), Duration::from_millis(10))
            .without_jitter();
        let err = retry_with_backoff(&policy, || {
            let attempts = &attempts;
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(ConnectorError::transport("always down"))
            }
        })
        .await
        .expect_err("attempts exhausted");
        assert!(matches!(err, ConnectorError::Transport(_)));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_does_not_retry_permission_errors() {
        let attempts = AtomicU32::new(0);
        let policy = RetryPolicy::new(5, Duration::from_millis(10), Duration::from_millis(10))
            .without_jitter();
        let err = retry_with_backoff(&policy, || {
            let attempts = &attempts;
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(ConnectorError::permission_required(
                    "req-1",
                    "connector.connect",
                    "connector:mock",
                    "Connect mock",
                ))
            }
        })
        .await
        .expect_err("permission errors are final");
        assert!(matches!(err, ConnectorError::PermissionRequired { .. }));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limiter_enforces_burst_and_refill() {
        let limiter = RateLimiter::new(RateLimitPolicy::new(2, Duration::from_secs(1)));
        limiter.acquire().await;
        limiter.acquire().await;
        // Both burst tokens consumed; the third acquire waits for a refill.
        let start = Instant::now();
        limiter.acquire().await;
        assert!(start.elapsed() >= Duration::from_millis(999));
    }

    #[tokio::test]
    async fn collect_pages_stops_at_last_page_and_cap() {
        let policy = PaginationPolicy::new(3, 2);
        let pages = collect_pages::<u32, ConnectorError, _, _>(&policy, |token| async move {
            match token.as_deref() {
                None => Ok(Page {
                    items: vec![1, 2],
                    next_page_token: Some("p2".to_string()),
                }),
                Some("p2") => Ok(Page {
                    items: vec![3],
                    next_page_token: None,
                }),
                _ => Err(ConnectorError::internal("unexpected token")),
            }
        })
        .await
        .expect("pagination succeeds");
        assert_eq!(pages, vec![1, 2, 3]);

        // A service that never ends is capped at max_pages.
        let endless = collect_pages::<u32, ConnectorError, _, _>(&policy, |token| async move {
            Ok(Page {
                items: vec![1],
                next_page_token: Some(token.unwrap_or_else(|| "p1".to_string())),
            })
        })
        .await
        .expect("capped pagination succeeds");
        assert_eq!(endless.len(), 3);
    }
}
