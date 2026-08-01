//! Tool operations handle, decoupled from the diagnostics facade (#406).
//!
//! Before this split, the runtime's tool surface (`list_tools`,
//! `search_tools`, `call_tool`, `invalidate_tool_index`) lived on
//! [`crate::diagnostics::EneDiagnostics`], which is documented as an
//! *opt-in diagnostics facade* — observability, not control. [`ToolHandle`]
//! gives tool operations their own handle, obtained via
//! [`crate::EneHandle::tools`], mirroring the `sessions()` / `candidates()` /
//! `vision()` split (#271).
//!
//! ## Why tool operations stay in the actor mailbox
//!
//! Unlike the read-only session/candidate/vision handles — which bypass the
//! actor mailbox entirely because they touch only `ene-store` / the vision
//! model — every tool operation here deliberately routes through
//! [`crate::handle::EneCommand`]:
//!
//! - **Admission caps (Stage 8)**: `search_tools` and `call_tool` run on the
//!   actor's bounded `search_tasks` / `call_tool_tasks` `JoinSet`s and must
//!   keep reporting [`EneRuntimeError::Busy`] when at capacity.
//!   The caps are actor-owned, so the work cannot move out of the mailbox.
//! - **Registry freshness**: the actor's `Arc<dyn ToolRegistry>` is swapped
//!   on plugin-host reconfiguration (`UpdateFeatureSettings` →
//!   `PluginHostReconfigured`). A mailbox-bypassing handle would need the
//!   actor to write through a shared `ArcSwap`-style container on every
//!   swap, plus a second container for the actor-owned `tool_rag` that
//!   `invalidate_tool_index` clears. That is a larger refactor with no
//!   observed head-of-line-blocking problem behind it — tool commands are
//!   infrequent and already bounded by admission control.
//!
//! The conservative choice is therefore to keep routing through the mailbox
//! and treat [`ToolHandle`] as the API-shape fix (control surface off the
//! diagnostics facade), not a transport optimization.

use crate::error::EneRuntimeError;
use crate::handle::EneCommand;
use crate::public_api::PublicApiError;
use ene_plugin_proto::ToolSpec;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// Handle for tool registry operations (list / search / call / invalidate).
///
/// Obtained via [`crate::EneHandle::tools`]. Cheap to clone (wraps a single
/// `Arc`d command sender). Routes through the actor mailbox — see the
/// module docs for why.
#[derive(Clone)]
pub struct ToolHandle {
    cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
}

impl ToolHandle {
    pub(crate) fn new(cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>) -> Self {
        Self { cmd_tx }
    }

    /// List available tools from the registry (includes the built-in
    /// `system.search_tools` entry).
    pub async fn list_tools(&self) -> Result<Vec<ToolSpec>, PublicApiError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::ListTools { reply: tx })
            .map_err(|_| PublicApiError::ActorDead)?;
        rx.await.map_err(|_| PublicApiError::ActorDead)
    }

    /// Search tools in the registry using RAG if available.
    ///
    /// # Errors
    ///
    /// Returns [`EneRuntimeError::Busy`] when the actor's
    /// tool-search task set is at capacity (Stage 8).
    pub async fn search_tools(&self, query: String) -> Result<Vec<ToolSpec>, EneRuntimeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::SearchTools { query, reply: tx })
            .map_err(|_| EneRuntimeError::ChannelClosed)?;
        rx.await.map_err(|_| EneRuntimeError::ChannelClosed)?
    }

    /// Call a tool directly by name with JSON-encoded arguments.
    pub async fn call_tool(
        &self,
        name: String,
        arguments: String,
    ) -> Result<String, EneRuntimeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::CallTool {
                name,
                arguments,
                turn: None,
                reply: tx,
            })
            .map_err(|_| EneRuntimeError::ChannelClosed)?;
        rx.await.map_err(|_| EneRuntimeError::ChannelClosed)?
    }

    /// Invalidate the Tool RAG index, forcing re-embedding on next query.
    pub fn invalidate_index(&self) -> Result<(), PublicApiError> {
        self.cmd_tx
            .send(EneCommand::InvalidateToolIndex)
            .map_err(|_| PublicApiError::ActorDead)
    }
}
