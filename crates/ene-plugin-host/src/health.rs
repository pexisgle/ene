//! Plugin health events surfaced to the diagnostics layer.
//!
//! The supervisor emits these whenever a plugin is detected unhealthy (hung or
//! crashed), restarted, recovered, or paused by the circuit breaker. The
//! runtime bridges them into diagnostic events so UI layers can react.
//! Statuses are stable English contracts.

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
    /// A plugin exceeded its restart budget and is disabled.
    Disabled {
        /// Plugin name.
        plugin: String,
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
            | Self::Disabled { plugin } => plugin,
        }
    }
}
