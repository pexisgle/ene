//! Persistent scheduler: timer task, late-execution policy, and config.

pub(crate) mod config;
pub(crate) mod task;

pub(crate) use config::SchedulerConfig;

use chrono::{DateTime, Utc};
use std::sync::Arc;

/// Wall-clock source for scheduler due-time evaluation.
///
/// Injectable so scheduler integration tests can pin deterministic
/// instants; the production default is the system wall clock.
pub(crate) type SchedulerClock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// The production scheduler clock: the system wall clock.
pub(crate) fn real_clock() -> SchedulerClock {
    Arc::new(Utc::now)
}
