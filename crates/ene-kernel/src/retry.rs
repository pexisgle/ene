//! Transient provider-call retry with a configured backoff sequence.

use std::fmt::Display;
use std::future::Future;
use std::time::Duration;

use crate::config::RetrySettings;

/// Whether a provider failure string should be retried.
///
/// Permanent auth / validation failures return immediately. Overload, timeout,
/// and gateway statuses use the configured backoff.
#[must_use]
pub fn is_retryable_provider_failure(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    if lower.contains("401")
        || lower.contains("403")
        || lower.contains("invalid api")
        || lower.contains("unauthorized")
        || lower.contains("not configured")
    {
        return false;
    }
    const MARKERS: &[&str] = &[
        "429",
        "502",
        "503",
        "504",
        "timeout",
        "timed out",
        "overload",
        "rate limit",
        "rate_limit",
        "unavailable",
        "econnreset",
        "broken pipe",
        "try again",
        "provider_overload",
        "provider failed",
    ];
    MARKERS.iter().any(|marker| lower.contains(marker))
}

/// Run `call` until it succeeds, a non-retryable error, or `max_attempts`.
pub async fn retry_call<T, E, F, Fut>(
    policy: &RetrySettings,
    is_retryable: impl Fn(&E) -> bool,
    mut call: F,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: Display,
{
    let max_attempts = policy.max_attempts.max(1);
    let mut attempt = 0_u32;
    loop {
        match call().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                let next = attempt.saturating_add(1);
                if !is_retryable(&err) || next >= max_attempts {
                    return Err(err);
                }
                let delay = backoff_for(policy, attempt);
                tracing::warn!(
                    attempt = next,
                    delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                    error = %err,
                    "retryable provider error; backing off"
                );
                tokio::time::sleep(delay).await;
                attempt = next;
            }
        }
    }
}

fn backoff_for(policy: &RetrySettings, retry_index: u32) -> Duration {
    let idx = usize::try_from(retry_index).unwrap_or(usize::MAX);
    let ms = policy
        .backoff_ms
        .get(idx)
        .copied()
        .or_else(|| policy.backoff_ms.last().copied())
        .unwrap_or(0);
    Duration::from_millis(u64::from(ms))
}

#[cfg(test)]
mod tests {
    use super::{is_retryable_provider_failure, retry_call};
    use crate::config::RetrySettings;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn fast_policy(max_attempts: u32) -> RetrySettings {
        RetrySettings {
            max_attempts,
            backoff_ms: vec![0],
        }
    }

    #[test]
    fn overload_is_retryable_and_auth_is_not() {
        assert!(is_retryable_provider_failure("429 too many requests"));
        assert!(is_retryable_provider_failure("provider failed"));
        assert!(!is_retryable_provider_failure("401 unauthorized"));
        assert!(!is_retryable_provider_failure(
            "chat model is not configured"
        ));
    }

    #[tokio::test]
    async fn succeeds_on_first_try() {
        let calls = AtomicU32::new(0);
        let result: Result<u32, String> = retry_call(
            &fast_policy(3),
            |_| true,
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok(7) }
            },
        )
        .await;
        assert_eq!(result.unwrap(), 7);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_transient_then_succeeds() {
        let calls = AtomicU32::new(0);
        let result: Result<u32, String> = retry_call(
            &fast_policy(3),
            |err: &String| is_retryable_provider_failure(err),
            || {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n < 2 {
                        Err("503 unavailable".to_owned())
                    } else {
                        Ok(9)
                    }
                }
            },
        )
        .await;
        assert_eq!(result.unwrap(), 9);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn permanent_failure_does_not_retry() {
        let calls = AtomicU32::new(0);
        let result: Result<u32, String> = retry_call(
            &fast_policy(5),
            |err: &String| is_retryable_provider_failure(err),
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Err("401 unauthorized".to_owned()) }
            },
        )
        .await;
        assert_eq!(result.unwrap_err(), "401 unauthorized");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn exhausts_attempts() {
        let calls = AtomicU32::new(0);
        let result: Result<u32, String> = retry_call(
            &RetrySettings {
                max_attempts: 2,
                backoff_ms: vec![0],
            },
            |_| true,
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Err("timeout".to_owned()) }
            },
        )
        .await;
        assert_eq!(result.unwrap_err(), "timeout");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
