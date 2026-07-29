//! # ene-tool-sdk
//!
//! Tool plugin authoring SDK: the [`ToolAction`] trait, provider adapters
//! ([`ActionSetProvider`], [`SingleActionProvider`]), and a [`prelude`] for
//! one-line imports.
//!
//! ## Modules
//!
//! - [`action`] — The unified [`ToolAction`] interface implemented by every tool
//! - [`provider`] — `ToolProvider` adapters over action sets
//!
//! ## Tool Design Philosophy
//!
//! Currently the tool plugin family uses two architectural patterns:
//!
//! 1. **Mega-tool approach** (fs, app, browser): A single binary per domain with multiple actions
//!    dispatched internally. Minimizes process overhead and IPC round-trips.
//! 2. **Individual-tool approach** (web, utility): Multiple smaller tools, each with a focused responsibility.
//!    Improves semantic matching precision in Tool RAG.
//!
//! A future unification to a single approach is possible but not yet decided.
//! When designing new tools, consider the trade-offs between startup overhead and
//! retrieval precision for your specific use case.
#![warn(missing_docs)]

/// Unified tool action interface.
pub mod action;
/// `ToolProvider` adapters over `Vec<Box<dyn ToolAction>>` (or a single action).
pub mod provider;

pub use action::{ToolAction, ToolSpecArgs};
pub use provider::ActionSetProvider;
pub use provider::SingleActionProvider;

/// Common imports for tool action files.
///
/// Use `use ene_tool_sdk::prelude::*;` to bring in the most frequently
/// needed items in a single line.
pub mod prelude {
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
}
