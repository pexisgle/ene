//! Plugin health events surfaced to the diagnostics layer.
//!
//! The supervisor emits these whenever a plugin is detected unhealthy (hung or
//! crashed), restarted, recovered, or paused by the circuit breaker. The
//! runtime bridges them into diagnostic events so UI layers can react.
//! Statuses are stable English contracts.

/// Why a plugin was permanently disabled ([`PluginHealthEvent::Disabled`]).
///
/// The [`Display`](std::fmt::Display) impl is the stable English code contract
/// surfaced to diagnostics consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisabledReason {
    /// The restart budget was exhausted (too many consecutive restarts).
    RestartBudgetExhausted,
    /// A restart-time binary checksum verification failed: the on-disk binary
    /// changed since it was pinned at startup.
    ChecksumMismatch,
}

impl std::fmt::Display for DisabledReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = match self {
            Self::RestartBudgetExhausted => "restart_budget_exhausted",
            Self::ChecksumMismatch => "checksum_mismatch",
        };
        f.write_str(code)
    }
}

/// A health/lifecycle event for a supervised plugin process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginHealthEvent {
    /// A plugin was detected unhealthy (hung or dead) and is about to restart.
    Unhealthy {
        /// Plugin name.
        plugin: String,
        /// Stable reason code: `"unresponsive"` (ping timeout) or `"dead"` (process exited).
        reason: String,
    },
    /// A plugin process is being restarted.
    Restarting {
        /// Plugin name.
        plugin: String,
        /// Restart attempt number (1-based).
        attempt: usize,
    },
    /// A plugin process was restarted and reconnected successfully.
    Restarted {
        /// Plugin name.
        plugin: String,
    },
    /// A previously unhealthy plugin responded to a health probe again.
    Recovered {
        /// Plugin name.
        plugin: String,
    },
    /// The circuit breaker opened after consecutive failures; calls are paused.
    CircuitOpened {
        /// Plugin name.
        plugin: String,
        /// Consecutive failure count that tripped the breaker.
        consecutive_failures: u32,
    },
    /// The circuit breaker closed after a successful call.
    CircuitClosed {
        /// Plugin name.
        plugin: String,
    },
    /// A plugin was permanently disabled and will not be restarted again.
    ///
    /// Emitted when the restart budget is exhausted or when a restart-time
    /// binary checksum verification fails (the on-disk binary changed since
    /// it was last verified). The plugin stays stopped; the user must
    /// intervene.
    Disabled {
        /// Plugin name.
        plugin: String,
        /// Why the plugin was disabled.
        reason: DisabledReason,
    },
    /// A plugin was not committed at startup because its required capabilities
    /// are not provided by any running plugin.
    ///
    /// Emitted for **hard** requirements (`soft: false`) that no provider
    /// satisfies. The plugin is shut down before supervision begins and never
    /// contributes tools or LLM providers; the user must activate a provider
    /// plugin and restart the host. A plugin whose only unmet requirements are
    /// soft starts normally and falls back to its built-in implementation.
    RequirementsUnmet {
        /// Plugin name.
        plugin: String,
        /// The unmet requirements, formatted as `name@range` (a trailing `?`
        /// marks a soft one), for diagnostics.
        unmet: Vec<String>,
    },
}

impl PluginHealthEvent {
    /// The plugin name this event concerns.
    pub const fn plugin(&self) -> &String {
        match self {
            Self::Unhealthy { plugin, .. }
            | Self::Restarting { plugin, .. }
            | Self::Restarted { plugin }
            | Self::Recovered { plugin }
            | Self::CircuitOpened { plugin, .. }
            | Self::CircuitClosed { plugin }
            | Self::Disabled { plugin, .. }
            | Self::RequirementsUnmet { plugin, .. } => plugin,
        }
    }
}
