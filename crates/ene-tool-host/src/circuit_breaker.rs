//! Per-tool circuit breaker (#238).
//!
//! Tracks consecutive call failures for a single tool. After a configurable
//! number of consecutive failures the breaker opens and subsequent calls are
//! rejected immediately (fail-fast) for a cooldown window, instead of
//! repeatedly hammering a broken tool. A successful call closes the breaker
//! and resets the failure count.

use std::time::{Duration, Instant};

/// Default number of consecutive failures before the breaker opens.
pub const DEFAULT_FAILURE_THRESHOLD: u32 = 5;
/// Default cooldown while the breaker is open.
pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30);

/// Circuit-breaker state for a single tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// Calls flow through normally.
    Closed,
    /// Calls are rejected until the cooldown elapses.
    Open,
}

/// Tracks consecutive failures and gates calls with a cooldown window.
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    failure_threshold: u32,
    cooldown: Duration,
    consecutive_failures: u32,
    opened_at: Option<Instant>,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new(DEFAULT_FAILURE_THRESHOLD, DEFAULT_COOLDOWN)
    }
}

impl CircuitBreaker {
    /// Creates a breaker that opens after `failure_threshold` consecutive
    /// failures and stays open for `cooldown`.
    pub const fn new(failure_threshold: u32, cooldown: Duration) -> Self {
        Self {
            failure_threshold,
            cooldown,
            consecutive_failures: 0,
            opened_at: None,
        }
    }

    /// Returns the current state, transitioning `Open` → `Closed` once the
    /// cooldown has elapsed (a half-open probe is allowed through).
    pub fn state(&mut self) -> BreakerState {
        match self.opened_at {
            Some(opened) if opened.elapsed() >= self.cooldown => {
                // Cooldown elapsed: allow a probe by closing the breaker.
                self.opened_at = None;
                BreakerState::Closed
            }
            Some(_) => BreakerState::Open,
            None => BreakerState::Closed,
        }
    }

    /// Whether a call should be rejected right now (breaker open).
    pub fn is_open(&mut self) -> bool {
        self.state() == BreakerState::Open
    }

    /// Records a successful call, closing the breaker and clearing failures.
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.opened_at = None;
    }

    /// Records a failed call, opening the breaker when the threshold is hit.
    /// Returns `true` when this failure caused the breaker to open.
    ///
    /// After the cooldown elapses and [`state`](Self::state) transitions
    /// the breaker back to `Closed`, `consecutive_failures` is **not**
    /// reset. This means the first call after a cooldown acts as a
    /// **half-open probe**: if it fails, `consecutive_failures` (already
    /// at `failure_threshold`) immediately re-opens the breaker for
    /// another full cooldown. A successful call (via
    /// [`record_success`](Self::record_success)) clears the counter and
    /// fully closes the breaker. This is intentional: a single probe
    /// failure after cooldown re-trips the breaker, preventing a
    /// cascade of calls to a still-broken tool.
    pub fn record_failure(&mut self) -> bool {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.opened_at.is_none() && self.consecutive_failures >= self.failure_threshold {
            self.opened_at = Some(Instant::now());
            return true;
        }
        false
    }

    /// Current consecutive-failure count.
    pub const fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_closed() {
        let mut breaker = CircuitBreaker::default();
        assert_eq!(breaker.state(), BreakerState::Closed);
        assert!(!breaker.is_open());
    }

    #[test]
    fn opens_after_threshold_failures() {
        let mut breaker = CircuitBreaker::new(3, Duration::from_secs(30));
        assert!(!breaker.record_failure());
        assert!(!breaker.record_failure());
        assert!(breaker.record_failure());
        assert_eq!(breaker.state(), BreakerState::Open);
        assert!(breaker.is_open());
    }

    #[test]
    fn success_resets_failures() {
        let mut breaker = CircuitBreaker::new(3, Duration::from_secs(30));
        let _ = breaker.record_failure();
        let _ = breaker.record_failure();
        breaker.record_success();
        assert_eq!(breaker.consecutive_failures(), 0);
        // Two more failures should not open (threshold is 3).
        assert!(!breaker.record_failure());
        assert!(!breaker.record_failure());
        assert_eq!(breaker.state(), BreakerState::Closed);
    }

    #[test]
    fn closes_after_cooldown() {
        let mut breaker = CircuitBreaker::new(1, Duration::from_millis(0));
        assert!(breaker.record_failure());
        // Zero cooldown means the next state check re-closes immediately.
        assert_eq!(breaker.state(), BreakerState::Closed);
    }
}
