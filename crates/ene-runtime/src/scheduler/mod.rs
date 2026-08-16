pub(crate) mod config;
pub(crate) mod task;

pub(crate) use config::SchedulerConfig;

use chrono::{DateTime, Utc};
use std::sync::Arc;

/// Injectable so scheduler integration tests can pin deterministic
/// instants; the production default is the system wall clock.
pub(crate) type SchedulerClock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

pub(crate) fn real_clock() -> SchedulerClock {
    Arc::new(Utc::now)
}
