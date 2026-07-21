//! Retry policy with exponential backoff and jitter for transient failures.

use std::time::Duration;

/// Configuration for retry behavior on transient errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Maximum number of attempts (including the initial call).
    pub max_attempts: u32,
    /// Base delay before the first retry.
    pub base_delay: Duration,
    /// Maximum delay cap for exponential growth.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    /// Computes the delay before attempt `retry_index` (0-indexed) with full jitter.
    fn delay_for_attempt(&self, retry_index: u32) -> Duration {
        let base_ms = self.base_delay.as_millis() as u64;
        let exp_ms = base_ms.saturating_mul(2u64.saturating_pow(retry_index));
        let capped = exp_ms.min(self.max_delay.as_millis() as u64);
        let jittered = if capped == 0 {
            0
        } else {
            rand::random_range(0..=capped)
        };
        Duration::from_millis(jittered)
    }

    /// Execute an async closure with retry on retryable errors.
    ///
    /// The `is_retryable` predicate determines whether a given error warrants
    /// a retry. Non-retryable errors are returned immediately.
    pub async fn run<F, Fut, T, E>(&self, is_retryable: impl Fn(&E) -> bool, f: F) -> Result<T, E>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
        E: std::fmt::Display,
    {
        self.run_with_retry_after(is_retryable, |_| None, f).await
    }

    /// Execute with an optional `Retry-After` hint (seconds) that overrides
    /// the computed backoff when present.
    pub async fn run_with_retry_after<F, Fut, T, E>(
        &self,
        is_retryable: impl Fn(&E) -> bool,
        retry_after_secs: impl Fn(&E) -> Option<u64>,
        mut f: F,
    ) -> Result<T, E>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
        E: std::fmt::Display,
    {
        let mut attempt: u32 = 0;
        loop {
            match f().await {
                Ok(val) => return Ok(val),
                Err(e) => {
                    let next = attempt.saturating_add(1);
                    if !is_retryable(&e) || next >= self.max_attempts {
                        return Err(e);
                    }
                    let delay = retry_after_secs(&e)
                        .map_or_else(|| self.delay_for_attempt(attempt), Duration::from_secs)
                        .min(self.max_delay);
                    tracing::warn!(
                        component = "retry",
                        attempt = next,
                        delay_ms = delay.as_millis() as u64,
                        error = %e,
                        "retryable error; backing off"
                    );
                    tokio::time::sleep(delay).await;
                    attempt = next;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn succeeds_on_first_try() {
        let policy = RetryPolicy::default();
        let calls = AtomicU32::new(0);
        let result: Result<u32, String> = policy
            .run(
                |_| true,
                || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok(42) }
                },
            )
            .await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_on_transient_then_succeeds() {
        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
        };
        let calls = AtomicU32::new(0);
        let result: Result<u32, String> = policy
            .run(
                |e: &String| e == "transient",
                || {
                    let n = calls.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if n < 2 {
                            Err("transient".to_string())
                        } else {
                            Ok(99)
                        }
                    }
                },
            )
            .await;
        assert_eq!(result.unwrap(), 99);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn non_retryable_fails_immediately() {
        let policy = RetryPolicy::default();
        let calls = AtomicU32::new(0);
        let result: Result<u32, String> = policy
            .run(
                |_| false,
                || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Err("permanent".to_string()) }
                },
            )
            .await;
        assert_eq!(result.unwrap_err(), "permanent");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn exhausts_attempts() {
        let policy = RetryPolicy {
            max_attempts: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(2),
        };
        let calls = AtomicU32::new(0);
        let result: Result<u32, String> = policy
            .run(
                |_| true,
                || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Err("always fails".to_string()) }
                },
            )
            .await;
        assert_eq!(result.unwrap_err(), "always fails");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn honors_retry_after_hint() {
        let policy = RetryPolicy {
            max_attempts: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_mins(1),
        };
        let calls = AtomicU32::new(0);
        // retry_after of 0 seconds keeps the test fast while exercising the override path.
        let result: Result<u32, String> = policy
            .run_with_retry_after(
                |_| true,
                |_| Some(0),
                || {
                    let n = calls.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if n == 0 {
                            Err("transient".to_string())
                        } else {
                            Ok(7)
                        }
                    }
                },
            )
            .await;
        assert_eq!(result.unwrap(), 7);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
