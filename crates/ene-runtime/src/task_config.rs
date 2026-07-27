//! Configuration for the [`crate::handle::actor::TurnActor`]'s bounded
//! background-task admission (Stage 8 of the concurrency redesign).
//!
//! Lives under the `tools.*` settings section — sibling of `tools.rag`
//! (see `ene_tool_rag::ToolRagConfig`) — since it governs tool-execution
//! *runtime* behavior (how many background tasks the actor keeps in
//! flight, how long a deferred poller runs), not the plugin process/IPC
//! layer that `plugins.*` (`ene_plugin_host::PluginConfig`) owns.
//!
//! Every actor `JoinSet` that accepts new background work is capped: once
//! full, admission is rejected rather than left to grow without bound (see
//! `crates/ene-runtime/src/handle/actor.rs`'s `admit_task`). The five caps
//! below are independently tunable because the five task sets have very
//! different risk profiles:
//!
//! - `call_tool_cap` — interactive `EneCommand::CallTool` /
//!   `CancelDeferredTool` calls. Driven by whoever holds an `EneHandle`
//!   (CLI/desktop UI, or an automated MCP-style caller), not by the LLM
//!   itself (in-turn tool calls run inline in the stream task, never
//!   through this `JoinSet`).
//! - `deferred_tool_cap` — long-lived background-tool pollers. Each one
//!   pins a task for up to `deferred_max_polls * 100ms` (~60s by
//!   default), so this is the most expensive slot per admitted task and
//!   the most exposed to a burst.
//! - `classifier_cap` / `memory_writer_cap` — post-turn affect
//!   classification and deferred memory writes. Losing admission here
//!   degrades quality (a turn's emotional/memory bookkeeping is skipped)
//!   but never correctness: the underlying work was already spawned by
//!   the stream task before its `JoinHandle` reached the actor, so a
//!   rejected admission only means the actor stops supervising it for
//!   panics, not that the work itself is cancelled.
//! - `search_cap` — `EneCommand::SearchTools` / tool-search jobs.

const fn default_call_tool_cap() -> usize {
    64
}

const fn default_deferred_tool_cap() -> usize {
    32
}

const fn default_classifier_cap() -> usize {
    16
}

const fn default_memory_writer_cap() -> usize {
    16
}

const fn default_search_cap() -> usize {
    16
}

const fn default_deferred_max_polls() -> u32 {
    600
}

ene_config::define_config!(
    settings,
    "tools",
    /// Bounded-task admission caps for the turn actor's background
    /// `JoinSet`s, plus the deferred-tool poll budget.
    ///
    /// See the module docs for the reasoning behind each default.
    pub struct ToolRuntimeConfig {
        /// Max concurrently in-flight direct `CallTool` /
        /// `CancelDeferredTool` tasks (`ENE_TOOLS__CALL_TOOL_CAP`).
        #[serde(default = "default_call_tool_cap")]
        pub call_tool_cap: usize = default_call_tool_cap(),
        /// Max concurrently in-flight deferred (background) tool pollers
        /// (`ENE_TOOLS__DEFERRED_TOOL_CAP`).
        #[serde(default = "default_deferred_tool_cap")]
        pub deferred_tool_cap: usize = default_deferred_tool_cap(),
        /// Max concurrently in-flight post-turn classifier supervisor
        /// tasks (`ENE_TOOLS__CLASSIFIER_CAP`).
        #[serde(default = "default_classifier_cap")]
        pub classifier_cap: usize = default_classifier_cap(),
        /// Max concurrently in-flight deferred memory-writer supervisor
        /// tasks (`ENE_TOOLS__MEMORY_WRITER_CAP`).
        #[serde(default = "default_memory_writer_cap")]
        pub memory_writer_cap: usize = default_memory_writer_cap(),
        /// Max concurrently in-flight tool-search jobs
        /// (`ENE_TOOLS__SEARCH_CAP`).
        #[serde(default = "default_search_cap")]
        pub search_cap: usize = default_search_cap(),
        /// Maximum poll iterations (at 100ms each) before a deferred
        /// (background) tool task is considered timed out (default 600 =
        /// 60s). Previously read directly from the
        /// `ENE_TOOLS__DEFERRED_MAX_POLLS` env var via `std::env::var`
        /// rather than through the config system; folded in here so it
        /// now also participates in defaults -> JSON -> env precedence
        /// and shows up in `settings.json`'s schema. The env var name is
        /// unchanged, so existing overrides keep working.
        #[serde(default = "default_deferred_max_polls")]
        pub deferred_max_polls: u32 = default_deferred_max_polls(),
    }
);

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "unit tests use unwrap/expect for concise assertions"
)]
mod tests {
    use super::ToolRuntimeConfig;
    use ene_config::EneConfig;

    #[test]
    fn defaults_match_documented_values() {
        let cfg = ToolRuntimeConfig::default();
        assert_eq!(cfg.call_tool_cap, 64);
        assert_eq!(cfg.deferred_tool_cap, 32);
        assert_eq!(cfg.classifier_cap, 16);
        assert_eq!(cfg.memory_writer_cap, 16);
        assert_eq!(cfg.search_cap, 16);
        assert_eq!(cfg.deferred_max_polls, 600);
    }

    #[test]
    fn round_trips_through_set_section_get_section() {
        let mut config = EneConfig::default();
        let custom = ToolRuntimeConfig {
            call_tool_cap: 4,
            deferred_tool_cap: 2,
            classifier_cap: 1,
            memory_writer_cap: 1,
            search_cap: 1,
            deferred_max_polls: 10,
        };
        config.set_section(&custom).expect("set_section succeeds");
        let read_back = config
            .get_section::<ToolRuntimeConfig>()
            .expect("get_section succeeds");
        assert_eq!(read_back.call_tool_cap, 4);
        assert_eq!(read_back.deferred_tool_cap, 2);
        assert_eq!(read_back.classifier_cap, 1);
        assert_eq!(read_back.memory_writer_cap, 1);
        assert_eq!(read_back.search_cap, 1);
        assert_eq!(read_back.deferred_max_polls, 10);
    }

    #[test]
    fn missing_section_falls_back_to_defaults() {
        let config = EneConfig::default();
        let read = config
            .get_section::<ToolRuntimeConfig>()
            .expect("get_section succeeds even when absent");
        assert_eq!(
            read.call_tool_cap,
            ToolRuntimeConfig::default().call_tool_cap
        );
    }
}
