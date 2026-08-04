//! Configuration for the persistent scheduler.
//!
//! Lives under the `scheduler.*` settings section, owned here because the
//! scheduler is a runtime concern (like `tools.*` for tool admission caps).

const fn default_enabled() -> bool {
    true
}

const fn default_late_grace_secs() -> u64 {
    60
}

const fn default_confirmation_timeout_secs() -> u64 {
    300
}

ene_config::define_config!(
    settings,
    "scheduler",
    /// Persistent scheduler policy.
    ///
    /// The scheduler only runs when the memory store is enabled (schedule
    /// definitions and run history persist in the store's database).
    pub struct SchedulerConfig {
        /// Master switch; when false no schedule fires
        /// (`ENE_SCHEDULER__ENABLED`).
        #[serde(default = "default_enabled")]
        pub enabled: bool = default_enabled(),
        /// A fire processed more than this many seconds after its scheduled
        /// time (suspend, clock jump, restart) is recorded `skipped_late` and
        /// not executed (`ENE_SCHEDULER__LATE_GRACE_SECS`).
        #[serde(default = "default_late_grace_secs")]
        pub late_grace_secs: u64 = default_late_grace_secs(),
        /// How long a scheduled run awaiting user confirmation may wait
        /// before it is recorded `timed_out` (`ENE_SCHEDULER__CONFIRMATION_TIMEOUT_SECS`).
        #[serde(default = "default_confirmation_timeout_secs")]
        pub confirmation_timeout_secs: u64 = default_confirmation_timeout_secs(),
    }
);

#[cfg(test)]
mod tests {
    use super::SchedulerConfig;

    #[test]
    fn defaults_match_documented_values() {
        let cfg = SchedulerConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.late_grace_secs, 60);
        assert_eq!(cfg.confirmation_timeout_secs, 300);
    }
}
