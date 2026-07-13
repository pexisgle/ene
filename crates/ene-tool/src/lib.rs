//! # ene-tool
//!
//! Facade crate that consolidates the tool ABI surface for API v2 (#135):
//!
//! | Layer | Trait / types | Source crate |
//! |---|---|---|
//! | Wire (IPC / sandbox) | [`ToolProvider`], [`IpcRequest`] / [`IpcResponse`], [`ToolSpec`] | `ene-tool-proto` |
//! | Host adapters | [`ActionSetProvider`], [`SingleActionProvider`], [`ToolAction`] | `ene-tool-common` |
//! | Derive | `#[derive(ToolSpec)]`, `#[derive(ToolAction)]` | `ene-tool-derive` |
//!
//! Host aggregation (`ToolRegistry`, MCP, composite, RAG) stays in
//! `ene-tool-host`. A full physical merge of proto+common+derive into this
//! crate (dropping the leaf crates) is a follow-up; this facade is the
//! supported import path for new tool binaries.
//!
//! ## Two-layer contract
//!
//! - **Wire:** tool binaries implement [`ToolProvider`] and speak
//!   handshake / list / call / permission-user-input continuation / shutdown.
//! - **Host:** `ene-tool-host::ToolRegistry` aggregates IPC + MCP.
//! - **Name collision** is a hard error at registry build / add time.
//! - [`ToolSpec`] is LLM-facing only: `name`, `description`, `parameters`.
//!
//! ```rust,ignore
//! use ene_tool::prelude::*;
//! ```

#![warn(missing_docs)]

/// Re-exports of the most common tool-authoring items.
pub mod prelude {
    #[doc(no_inline)]
    pub use ene_tool_common::{ActionSetProvider, SingleActionProvider, ToolAction, ToolSpecArgs};
    #[doc(no_inline)]
    pub use ene_tool_derive::{ToolAction as DeriveToolAction, ToolSpec as DeriveToolSpec};
    #[doc(no_inline)]
    pub use ene_tool_proto::{
        SandboxConfigData, ToolError, ToolName, ToolProvider, ToolSpec, run_tool_server,
    };
}

#[doc(no_inline)]
pub use ene_tool_common::{ActionSetProvider, SingleActionProvider, ToolAction, ToolSpecArgs};
#[doc(no_inline)]
pub use ene_tool_derive::{ToolAction as DeriveToolAction, ToolSpec as DeriveToolSpec};
#[doc(no_inline)]
pub use ene_tool_proto::{
    ActionSpec, EmbeddingField, HostRegistry, IPC_PROTOCOL_VERSION, IpcRequest, IpcResponse,
    KeywordSet, SandboxConfigData, SideEffects, ToolCategory, ToolError, ToolExample, ToolName,
    ToolProvider, ToolSpec, ToolVersion, run_tool_server,
};
