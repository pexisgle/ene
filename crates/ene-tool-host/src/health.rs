//! Tool health events surfaced to the diagnostics layer (#238).
//!
//! The supervisor emits these whenever a tool is detected unhealthy (hung or
//! crashed), restarted, recovered, or paused by the circuit breaker. The
//! runtime bridges them into [`ene_runtime`] `DiagnosticEvent`s so UI layers
//! can react. Statuses are stable English contracts.

/// A health/lifecycle event for a supervised tool process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolHealthEvent {
    /// A tool was detected unhealthy (hung or dead) and is about to restart.
    Unhealthy {
        /// Tool name.
        tool: String,
        /// Stable reason code: `"unresponsive"` (ping timeout) or `"dead"` (process exited).
        reason: String,
    },
    /// A tool process is being restarted.
    Restarting {
        /// Tool name.
        tool: String,
        /// Restart attempt number (1-based).
        attempt: usize,
    },
    /// A tool process was restarted and reconnected successfully.
    Restarted {
        /// Tool name.
        tool: String,
    },
    /// A previously unhealthy tool responded to a health probe again.
    Recovered {
        /// Tool name.
        tool: String,
    },
    /// The circuit breaker opened after consecutive failures; calls are paused.
    CircuitOpened {
        /// Tool name.
        tool: String,
        /// Consecutive failure count that tripped the breaker.
        consecutive_failures: u32,
    },
    /// The circuit breaker closed after a successful call.
    CircuitClosed {
        /// Tool name.
        tool: String,
    },
    /// A tool exceeded its restart budget and is disabled.
    Disabled {
        /// Tool name.
        tool: String,
    },
}

impl ToolHealthEvent {
    /// The tool name this event concerns.
    pub const fn tool(&self) -> &String {
        match self {
            Self::Unhealthy { tool, .. }
            | Self::Restarting { tool, .. }
            | Self::Restarted { tool }
            | Self::Recovered { tool }
            | Self::CircuitOpened { tool, .. }
            | Self::CircuitClosed { tool }
            | Self::Disabled { tool } => tool,
        }
    }
}
