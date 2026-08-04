//! Persistent scheduler: timer task, late-execution policy, and config.

pub(crate) mod config;
pub(crate) mod task;

pub(crate) use config::SchedulerConfig;
