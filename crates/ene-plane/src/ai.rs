use async_trait::async_trait;

use crate::request::AuthzRequest;

/// Auxiliary-LLM approval judgement (P-904).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiJudgement {
    pub allow: bool,
    pub reason: String,
}

/// `ai.tasks.approve` seam. Failure must fall back to popup, never auto-run.
#[async_trait]
pub trait ApproveModel: Send + Sync {
    async fn judge(&self, req: &AuthzRequest) -> Result<AiJudgement, String>;
}

/// Test stand-in that always returns the stored judgement.
#[derive(Debug, Clone)]
pub struct ScriptedAi {
    pub judgement: Result<AiJudgement, String>,
}

#[async_trait]
impl ApproveModel for ScriptedAi {
    async fn judge(&self, _req: &AuthzRequest) -> Result<AiJudgement, String> {
        self.judgement.clone()
    }
}
