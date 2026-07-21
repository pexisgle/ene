//! Provider health checks and failover monitoring (#175).
//!
//! Health checks probe a provider's `/models` endpoint with a short timeout
//! and **never** send user data. Results are cached in a [`ProviderHealthMonitor`]
//! with a configurable TTL so repeated turns do not re-probe on every call.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Outcome of a single provider health probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderHealthStatus {
    /// Provider responded successfully within the timeout.
    Healthy,
    /// Provider responded but with elevated latency (> 2× the median).
    Degraded {
        /// Measured round-trip latency.
        latency_ms: u64,
    },
    /// Provider rejected the API key (HTTP 401/403).
    AuthFailed,
    /// Provider is rate-limiting (HTTP 429).
    RateLimited,
    /// Provider is unreachable (network error, DNS, TLS, timeout).
    Unreachable,
    /// Provider returned an unexpected HTTP status.
    ServerError {
        /// HTTP status code.
        status: u16,
    },
    /// Health has not been checked yet.
    Unknown,
}

impl ProviderHealthStatus {
    /// Whether the provider is usable for a chat request right now.
    #[must_use]
    pub const fn is_available(&self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded { .. } | Self::Unknown)
    }

    /// Stable English status code for the diagnostics contract (#175).
    #[must_use]
    pub const fn status_code(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded { .. } => "degraded",
            Self::AuthFailed => "auth_failed",
            Self::RateLimited => "rate_limited",
            Self::Unreachable => "unreachable",
            Self::ServerError { .. } => "server_error",
            Self::Unknown => "unknown",
        }
    }
}

/// A single health probe result for one provider.
#[derive(Debug, Clone)]
pub struct ProviderHealthReport {
    /// Provider name (key in `ai.providers`).
    pub provider: String,
    /// Probe outcome.
    pub status: ProviderHealthStatus,
    /// Measured round-trip latency (0 if unreachable).
    pub latency_ms: u64,
    /// Human-readable error detail, if any.
    pub error: Option<String>,
    /// When the probe was performed.
    pub checked_at: Instant,
}

/// Errors that can occur during a health probe.
#[derive(Debug, thiserror::Error)]
pub enum HealthCheckError {
    /// The HTTP client could not be initialized.
    #[error("health check client init failed: {0}")]
    ClientInit(String),
    /// The probe request failed at the transport level.
    #[error("health check request failed: {0}")]
    Request(String),
}

/// Probe a provider's `/models` endpoint with a timeout.
///
/// Sends **no** user data — only an authenticated `GET` to verify
/// connectivity and credential validity. Returns a [`ProviderHealthReport`].
pub async fn check_provider_health(
    provider_name: &str,
    base_url: &str,
    api_key: &str,
    timeout: Duration,
) -> ProviderHealthReport {
    let start = Instant::now();

    if api_key.trim().is_empty() {
        return ProviderHealthReport {
            provider: provider_name.to_string(),
            status: ProviderHealthStatus::AuthFailed,
            latency_ms: 0,
            error: Some("API key is empty".to_string()),
            checked_at: start,
        };
    }

    let client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(c) => c,
        Err(e) => {
            return ProviderHealthReport {
                provider: provider_name.to_string(),
                status: ProviderHealthStatus::Unreachable,
                latency_ms: 0,
                error: Some(format!("client init: {e}")),
                checked_at: start,
            };
        }
    };

    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let result = client.get(url).bearer_auth(api_key).send().await;

    let latency_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(response) => {
            let status_code = response.status().as_u16();
            let status = if response.status().is_success() {
                ProviderHealthStatus::Healthy
            } else {
                match status_code {
                    401 | 403 => ProviderHealthStatus::AuthFailed,
                    429 => ProviderHealthStatus::RateLimited,
                    _ => ProviderHealthStatus::ServerError {
                        status: status_code,
                    },
                }
            };
            let error = if response.status().is_success() {
                None
            } else {
                let body = response.text().await.unwrap_or_default();
                let snippet: String = body.chars().take(200).collect();
                Some(format!("HTTP {status_code}: {snippet}"))
            };
            ProviderHealthReport {
                provider: provider_name.to_string(),
                status,
                latency_ms,
                error,
                checked_at: start,
            }
        }
        Err(e) => ProviderHealthReport {
            provider: provider_name.to_string(),
            status: ProviderHealthStatus::Unreachable,
            latency_ms,
            error: Some(format!("request failed: {e}")),
            checked_at: start,
        },
    }
}

/// A recorded fallback event.
#[derive(Debug, Clone)]
pub struct FallbackRecord {
    /// Provider that failed.
    pub from: String,
    /// Provider that was selected instead.
    pub to: String,
    /// Reason for the fallback.
    pub reason: String,
    /// When the fallback occurred.
    pub at: Instant,
}

/// In-memory health monitor with TTL caching and fallback history (#175).
///
/// Thread-safe via `Arc<Mutex<..>>`. The runtime actor holds one instance
/// and shares it with the diagnostics facade for `/doctor` queries.
#[derive(Clone)]
pub struct ProviderHealthMonitor {
    inner: Arc<Mutex<MonitorInner>>,
}

struct MonitorInner {
    /// Cached health reports keyed by provider name.
    reports: HashMap<String, ProviderHealthReport>,
    /// How long a cached report is considered fresh.
    ttl: Duration,
    /// Recent fallback events (bounded ring buffer).
    fallback_history: Vec<FallbackRecord>,
    /// Maximum fallback records to retain.
    max_history: usize,
}

impl std::fmt::Debug for ProviderHealthMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.lock();
        f.debug_struct("ProviderHealthMonitor")
            .field("cached_providers", &inner.reports.len())
            .field("fallback_count", &inner.fallback_history.len())
            .finish()
    }
}

impl ProviderHealthMonitor {
    /// Create a new monitor with the given cache TTL and history capacity.
    #[must_use]
    pub fn new(ttl: Duration, max_history: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MonitorInner {
                reports: HashMap::new(),
                ttl,
                fallback_history: Vec::new(),
                max_history,
            })),
        }
    }

    /// Record a health probe result, replacing any previous entry.
    pub fn record(&self, report: ProviderHealthReport) {
        let mut inner = self.inner.lock();
        inner.reports.insert(report.provider.clone(), report);
    }

    /// Get the cached health status for a provider, if fresh.
    ///
    /// Returns `None` when no report exists or the cached report has expired.
    pub fn get_fresh(&self, provider: &str) -> Option<ProviderHealthReport> {
        let inner = self.inner.lock();
        inner.reports.get(provider).and_then(|r| {
            if r.checked_at.elapsed() < inner.ttl {
                Some(r.clone())
            } else {
                None
            }
        })
    }

    /// Get the cached health status regardless of freshness.
    pub fn get_any(&self, provider: &str) -> Option<ProviderHealthReport> {
        let inner = self.inner.lock();
        inner.reports.get(provider).cloned()
    }

    /// Record a fallback event.
    pub fn record_fallback(&self, from: &str, to: &str, reason: &str) {
        let mut inner = self.inner.lock();
        if inner.fallback_history.len() >= inner.max_history {
            inner.fallback_history.remove(0);
        }
        inner.fallback_history.push(FallbackRecord {
            from: from.to_string(),
            to: to.to_string(),
            reason: reason.to_string(),
            at: Instant::now(),
        });
    }

    /// Snapshot of recent fallback events (newest last).
    pub fn fallback_history(&self) -> Vec<FallbackRecord> {
        let inner = self.inner.lock();
        inner.fallback_history.clone()
    }

    /// Snapshot of all cached health reports.
    pub fn all_reports(&self) -> Vec<ProviderHealthReport> {
        let inner = self.inner.lock();
        inner.reports.values().cloned().collect()
    }
}

impl Default for ProviderHealthMonitor {
    fn default() -> Self {
        Self::new(Duration::from_mins(1), 32)
    }
}

/// Result of a failover selection.
#[derive(Debug, Clone)]
pub struct FailoverSelection {
    /// The selected candidate.
    pub candidate: crate::resolve::ChatCandidate,
    /// Providers that were tried and skipped before the selection, with reasons.
    pub skipped: Vec<(String, String)>,
    /// Whether a fallback occurred (selected is not the first candidate).
    pub fell_back: bool,
}

/// Select the first healthy chat candidate in priority order (#175).
///
/// Probes each candidate's `/models` endpoint (using the monitor's TTL cache
/// to avoid redundant probes) and returns the first one whose health status is
/// [`ProviderHealthStatus::is_available`]. If a fallback occurs, the event is
/// recorded in the monitor. Sends **no** user data during probes.
///
/// Returns `None` only when there are no candidates at all.
pub async fn select_healthy_chat(
    candidates: &[crate::resolve::ChatCandidate],
    monitor: &ProviderHealthMonitor,
    timeout: Duration,
) -> Option<FailoverSelection> {
    if candidates.is_empty() {
        return None;
    }

    let mut skipped = Vec::new();

    for (index, candidate) in candidates.iter().enumerate() {
        // Use a fresh cached report if available, otherwise probe.
        let report = if let Some(cached) = monitor.get_fresh(&candidate.provider) {
            cached
        } else {
            let fresh = check_provider_health(
                &candidate.provider,
                &candidate.base_url,
                &candidate.api_key,
                timeout,
            )
            .await;
            monitor.record(fresh.clone());
            fresh
        };

        if report.status.is_available() {
            let fell_back = index > 0;
            if fell_back && let Some(primary) = candidates.first() {
                let reason = skipped
                    .iter()
                    .map(|(p, r)| format!("{p}: {r}"))
                    .collect::<Vec<_>>()
                    .join("; ");
                monitor.record_fallback(&primary.provider, &candidate.provider, &reason);
            }
            return Some(FailoverSelection {
                candidate: candidate.clone(),
                skipped,
                fell_back,
            });
        }

        let reason = report
            .error
            .clone()
            .unwrap_or_else(|| format!("{:?}", report.status));
        skipped.push((candidate.provider.clone(), reason));
    }

    // No candidate was healthy. Fall back to the first candidate anyway so
    // the turn can attempt to proceed (the provider may recover mid-request),
    // but record that every probe failed.
    let primary = candidates.first()?;
    let reason = skipped
        .iter()
        .map(|(p, r)| format!("{p}: {r}"))
        .collect::<Vec<_>>()
        .join("; ");
    tracing::warn!(
        component = "health",
        reason = %reason,
        "all providers unhealthy; using primary candidate anyway"
    );
    Some(FailoverSelection {
        candidate: primary.clone(),
        skipped,
        fell_back: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_status_availability() {
        assert!(ProviderHealthStatus::Healthy.is_available());
        assert!(ProviderHealthStatus::Degraded { latency_ms: 500 }.is_available());
        assert!(ProviderHealthStatus::Unknown.is_available());
        assert!(!ProviderHealthStatus::AuthFailed.is_available());
        assert!(!ProviderHealthStatus::RateLimited.is_available());
        assert!(!ProviderHealthStatus::Unreachable.is_available());
        assert!(!ProviderHealthStatus::ServerError { status: 500 }.is_available());
    }

    #[test]
    fn monitor_records_and_retrieves() {
        let monitor = ProviderHealthMonitor::new(Duration::from_mins(1), 8);
        let report = ProviderHealthReport {
            provider: "default".to_string(),
            status: ProviderHealthStatus::Healthy,
            latency_ms: 42,
            error: None,
            checked_at: Instant::now(),
        };
        monitor.record(report.clone());
        let fresh = monitor.get_fresh("default");
        assert!(fresh.is_some());
        assert_eq!(fresh.unwrap().latency_ms, 42);
        assert!(monitor.get_fresh("nonexistent").is_none());
    }

    #[test]
    fn monitor_fallback_history_bounded() {
        let monitor = ProviderHealthMonitor::new(Duration::from_mins(1), 3);
        for i in 0..5 {
            monitor.record_fallback(&format!("p{i}"), "fallback", "test");
        }
        let history = monitor.fallback_history();
        assert_eq!(history.len(), 3);
        assert_eq!(history.first().map(|r| r.from.as_str()), Some("p2"));
        assert_eq!(history.get(2).map(|r| r.from.as_str()), Some("p4"));
    }

    #[test]
    fn monitor_expired_report_not_fresh() {
        let monitor = ProviderHealthMonitor::new(Duration::from_millis(0), 8);
        let checked_at = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let report = ProviderHealthReport {
            provider: "default".to_string(),
            status: ProviderHealthStatus::Healthy,
            latency_ms: 10,
            error: None,
            checked_at,
        };
        monitor.record(report);
        assert!(monitor.get_fresh("default").is_none());
        assert!(monitor.get_any("default").is_some());
    }
}
