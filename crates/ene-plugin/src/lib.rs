//! # ene-plugin
//!
//! Plugin authoring facade for the ene unified plugin system.
//!
//! This crate provides the [`ToolPlugin`], [`LlmPlugin`], and [`EmbedPlugin`]
//! traits and the [`run_plugin_server`] entry point for building plugin
//! binaries. A plugin can implement any subset of these traits; the server
//! dispatches requests to the appropriate trait via [`PluginDispatch`].
//!
//! ## Relationship to other crates
//!
//! - [`ene-plugin-proto`](ene_plugin_proto) — wire protocol definitions
//!   (protocol v4), framing helpers, and transport layer.
//! - `ene-plugin-host` — host-side process supervision and capability
//!   routing (consumes plugins, does not author them).
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use async_trait::async_trait;
//! use std::sync::Arc;
//! use ene_plugin::{ToolPlugin, ToolPluginCapabilities, PluginDispatch, PluginError};
//!
//! struct MyTool;
//!
//! #[async_trait]
//! impl ToolPlugin for MyTool {
//!     fn tool_capabilities(&self) -> ToolPluginCapabilities {
//!         ToolPluginCapabilities { tool_count: 0 }
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), PluginError> {
//!     ene_plugin::run_plugin_server(
//!         PluginDispatch::new(Some(Arc::new(MyTool)), None, None, None, None),
//!     ).await
//! }
//! ```
#![warn(missing_docs)]
#![cfg_attr(
    test,
    expect(clippy::unwrap_used, reason = "unit tests use unwrap for assertions")
)]

/// Unified tool action interface (merged from former `ene-tool-sdk`).
pub mod action;
/// Compatibility adapter for wrapping legacy [`ToolProvider`] as [`ToolPlugin`].
pub mod compat;
/// Plugin trait and streaming chunk types.
pub mod plugin;
/// Plugin IPC server and dispatch loop.
pub mod server;
/// `ToolProvider` adapters over `Vec<Box<dyn ToolAction>>` (or a single action).
pub mod tool_provider;

pub use action::{ToolAction, ToolSpecArgs};
pub use compat::ToolProviderPlugin;
pub use plugin::{
    EmbedPlugin, LlmPlugin, PluginStream, PluginStreamChunk, SttPlugin, ToolPlugin,
    ToolPluginCapabilities, TtsPlugin,
};
pub use server::{PluginDispatch, run_plugin_server};
pub use tool_provider::{ActionSetProvider, SingleActionProvider};

// Re-export key types from ene-plugin-proto so plugin authors only need
// to depend on `ene-plugin` for the full authoring surface.
pub use ene_plugin_proto::{
    ConcurrencyHint, LlmProviderSpec, PLUGIN_IPC_PROTOCOL_VERSION, PluginCapabilities, PluginError,
    PluginIpcRequest, PluginIpcResponse, SttProviderSpec, TtsProviderSpec, VersionRange,
};
/// Cross-platform IPC transport (re-exported from `ene-plugin-proto`).
pub use ene_plugin_proto::{IpcListener, IpcStream, cleanup_path};
/// Shared tool types (re-exported from `ene-plugin-proto`).
pub use ene_plugin_proto::{ToolError, ToolResult, ToolSpec};

// Re-export additional tool-proto types used by the server.
pub use ene_plugin_proto::{DeferredStatus, SandboxConfigData};

/// One-line import of the plugin authoring surface, including local-inference
/// discipline for plugins that run their own model in-process.
///
/// ## Why `ene-infer` is re-exported here
///
/// The concurrency redesign this crate is part of makes plugin-supplied
/// providers safe *from the host's side*: [`ConcurrencyHint`] lets a plugin
/// declare how many concurrent jobs it can take, and the host enforces that
/// with admission control (see `ene-plugin-host::ipc_provider`). But a
/// plugin that does its own local inference (llama.cpp, whisper.cpp, a
/// local TTS engine) still has to get concurrency right *inside its own
/// process* — the host can only throttle how many requests it sends, not
/// how the plugin's own code juggles them once they arrive.
///
/// [`ene_infer`] is the framework the host itself would reach for to run a
/// synchronous, single-threaded local model safely: a dedicated worker
/// thread, a bounded queue, cooperative cancellation, and panic recovery,
/// all behind a plain [`LocalModel`] trait instead of hand-rolled
/// `spawn_blocking`/`block_in_place` concurrency. Re-exporting its types
/// here means a plugin author reaches for the *same* discipline the host
/// uses, via one `use ene_plugin::prelude::*;`, instead of this redesign
/// simply relocating the bug across the process boundary.
///
/// `ene-infer` is a leaf crate (only `tokio`, `tokio-util`, `thiserror`,
/// `tracing`) with no AI, DB, or wire-ABI concepts of its own, so this
/// dependency does not cross any architecture boundary documented in
/// `AGENTS.md`.
///
/// ## Submodules
///
/// The prelude is organized into two submodules for clarity:
///
/// - [`prelude::tool`] — Tool plugin authoring: `ToolAction`, `ActionSetProvider`,
///   derive macros, and related types.
/// - [`prelude::provider`] — Provider plugin authoring: `LlmPlugin`, `TtsPlugin`,
///   `SttPlugin`, `EmbedPlugin`, server entry point, and inference discipline.
///
/// A backward-compatible glob re-export at the prelude root means
/// `use ene_plugin::prelude::*;` still brings in everything.
pub mod prelude {
    /// Tool plugin authoring imports.
    pub mod tool {
        #[doc(no_inline)]
        pub use async_trait::async_trait;
        #[doc(no_inline)]
        pub use ene_plugin_proto::ToolError;
        #[doc(no_inline)]
        pub use ene_tool_macros::{ToolAction, ToolSpec, tool_action};
        #[doc(no_inline)]
        pub use schemars::JsonSchema;
        #[doc(no_inline)]
        pub use serde::Deserialize;

        #[doc(no_inline)]
        pub use crate::ActionSetProvider;
        #[doc(no_inline)]
        pub use crate::SingleActionProvider;
        #[doc(no_inline)]
        pub use crate::ToolAction as _;
        #[doc(no_inline)]
        pub use crate::ToolSpecArgs;
    }

    /// Provider plugin authoring imports (LLM, TTS, STT, embedding).
    pub mod provider {
        #[doc(no_inline)]
        pub use ene_infer::{
            EngineConfig, EngineError, EngineHandle, JobContext, LocalModel, StopReason,
        };

        #[doc(no_inline)]
        pub use crate::{
            ConcurrencyHint, EmbedPlugin, LlmPlugin, LlmProviderSpec, PluginDispatch, PluginError,
            PluginStream, PluginStreamChunk, SttPlugin, SttProviderSpec, ToolPlugin,
            ToolPluginCapabilities, TtsPlugin, TtsProviderSpec, run_plugin_server,
        };
    }

    // Backward-compatible glob re-exports so `use ene_plugin::prelude::*;`
    // continues to work for both tool and provider plugin authors.
    #[doc(no_inline)]
    pub use provider::*;
    #[doc(no_inline)]
    pub use tool::*;
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "unit tests use unwrap for concise failure messages"
)]
mod prelude_tests {
    // Every item this test needs comes from the prelude's single glob
    // import, proving a plugin author authoring local inference really can
    // reach the host's own concurrency discipline via one `use` line.
    use crate::prelude::*;

    #[derive(Debug, thiserror::Error)]
    #[error("doubling model error")]
    struct DoublingModelError;

    /// The simplest possible `LocalModel`: doubles an integer. Stands in for
    /// a real local-inference engine (llama.cpp, whisper.cpp, ...) just to
    /// prove the prelude's re-exports are sufficient to wire one up.
    struct DoublingModel;

    impl LocalModel for DoublingModel {
        type Request = i32;
        type Response = i32;
        type Error = DoublingModelError;

        fn engine_name(&self) -> &'static str {
            "doubling-test-model"
        }

        fn run(
            &mut self,
            req: Self::Request,
            _ctx: &JobContext,
        ) -> Result<Self::Response, Self::Error> {
            Ok(req * 2)
        }
    }

    #[tokio::test]
    async fn prelude_reexports_are_sufficient_to_run_a_local_model() {
        let handle: EngineHandle<DoublingModel> =
            EngineHandle::spawn(|| Ok(DoublingModel), EngineConfig::default());
        let result = handle
            .submit(21, tokio_util::sync::CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result, 42);
    }

    /// A plugin declaring a permissive concurrency hint, using only the
    /// prelude's re-export of [`ConcurrencyHint`].
    #[test]
    fn prelude_reexports_concurrency_hint() {
        let hint = ConcurrencyHint {
            max_in_flight: 4,
            queue_depth: 8,
        };
        assert_eq!(hint.max_in_flight, 4);
        // The default (not overridden here) stays serial regardless.
        assert_eq!(ConcurrencyHint::default().max_in_flight, 1);
    }
}
