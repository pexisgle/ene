use crate::error::KernelError;
use async_trait::async_trait;
use serde_json::Value;

/// Result of a surface-lane tool request. Side-effect tools must return
/// [`SurfaceToolOutcome::Delegated`] and must not have been executed.
#[derive(Debug, Clone, PartialEq)]
pub enum SurfaceToolOutcome {
    Result(Value),
    Delegated {
        speech: String,
        job_id: String,
    },
    /// Approval plane refused the call; `reason` is user-displayable.
    Denied {
        reason: String,
    },
}

/// Host callback used by the dialogue lane. Implemented in `ene-work`.
#[async_trait]
pub trait SurfaceRouter: Send + Sync {
    async fn on_tool(
        &self,
        name: &str,
        args: Value,
        step: u32,
    ) -> Result<SurfaceToolOutcome, KernelError>;
}
