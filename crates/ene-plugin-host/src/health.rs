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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginHealthEvent {
    Unhealthy {
        plugin: String,
        /// Stable reason code: `"unresponsive"` (ping timeout) or `"dead"` (process exited).
        reason: String,
    },
    Restarting {
        plugin: String,
        /// Restart attempt number (1-based).
        attempt: usize,
    },
    Restarted {
        plugin: String,
    },
    Recovered {
        plugin: String,
    },
    CircuitOpened {
        plugin: String,
        consecutive_failures: u32,
    },
    CircuitClosed {
        plugin: String,
    },
    /// A plugin was permanently disabled and will not be restarted again.
    ///
    /// Emitted when the restart budget is exhausted or when a restart-time
    /// binary checksum verification fails (the on-disk binary changed since
    /// it was last verified). The plugin stays stopped; the user must
    /// intervene.
    Disabled {
        plugin: String,
        reason: DisabledReason,
    },
    /// A plugin was not registered because its hard capability requirements
    /// have no provider (see `capability_registry`).
    ///
    /// Emitted once at startup, before the plugin's tools or providers would
    /// be registered. The plugin process is not supervised; recovery is a
    /// host restart (or a `plugins.list` reconfiguration with a provider
    /// present). Soft requirements never produce this event — the plugin
    /// starts and is expected to fall back.
    RequirementsUnmet {
        plugin: String,
        /// The unmet hard requirements, as `name@[^]major` strings.
        requirements: Vec<String>,
    },
}

impl PluginHealthEvent {
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
