use std::sync::{Arc, Weak};

use async_trait::async_trait;
use ene_kernel::AiTaskKind;
use ene_plane::{AiJudgement, ApproveModel, AuthzRequest};
use ene_plugin_ipc::{LlmGenerateRequest, LlmMessage, LlmRole, ProviderAuth};

use crate::CoreDaemon;

/// `ai.tasks.approve` (falls back to chat). Failure is the caller's popup path.
pub struct SeamedApprove {
    core: Weak<CoreDaemon>,
}

impl SeamedApprove {
    #[must_use]
    pub fn new(core: &Arc<CoreDaemon>) -> Self {
        Self {
            core: Arc::downgrade(core),
        }
    }
}

#[async_trait]
impl ApproveModel for SeamedApprove {
    async fn judge(&self, req: &AuthzRequest) -> Result<AiJudgement, String> {
        let core = self
            .core
            .upgrade()
            .ok_or_else(|| "core stopped".to_owned())?;
        let task = crate::seam_client::resolve_task(&core, AiTaskKind::Approve)
            .ok_or_else(|| "approve task is not configured".to_owned())?;
        let request = LlmGenerateRequest {
            messages: vec![
                LlmMessage::new(
                    LlmRole::System,
                    "You are the approval helper. Reply with a JSON object only: \
                     {\"allow\": boolean, \"reason\": string}. \
                     allow=true only when the tool call is clearly bounded and reversible. \
                     Never allow destructive filesystem or credential access. \
                     If a filesystem target is present, never allow a path outside the workspace.",
                ),
                LlmMessage::new(LlmRole::User, {
                    let target = if req.target.is_empty() {
                        "(none)"
                    } else {
                        req.target.as_str()
                    };
                    let in_workspace = if req.target.is_empty() {
                        "n/a"
                    } else if req.in_workspace {
                        "true"
                    } else {
                        "false"
                    };
                    format!(
                        "tool: {}\ntarget: {target}\nside_effects: {}\nin_workspace: {in_workspace}\nsensitivity: {:?}",
                        req.tool,
                        req.side_effects.join(","),
                        req.sensitivity
                    )
                }),
            ],
            tools: Vec::new(),
            model: task.binding.model.clone(),
            max_tokens: task.binding.max_tokens.or(Some(256)),
            base_url: task.binding.base_url.clone(),
            auth: ProviderAuth {
                api_key: task.api_key.clone(),
            },
        };
        let generation = crate::seam_client::generate_llm(&core, &task, request).await?;
        parse_ai_judgement(&generation.text)
    }
}

pub(crate) fn parse_ai_judgement(raw: &str) -> Result<AiJudgement, String> {
    let trimmed = raw.trim();
    let trimmed = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed)
        .trim();
    let trimmed = trimmed.strip_suffix("```").unwrap_or(trimmed).trim();
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|err| format!("approve json: {err}"))?;
    let allow = value
        .get("allow")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| "approve json missing allow".to_owned())?;
    let reason = value
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim();
    if reason.is_empty() {
        return Err("approve json missing reason".to_owned());
    }
    Ok(AiJudgement {
        allow,
        reason: reason.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::parse_ai_judgement;

    #[test]
    fn parse_ai_judgement_reads_allow_and_reason() {
        let ok = parse_ai_judgement(
            "```json\n{\"allow\": true, \"reason\": \"workspace write looks safe\"}\n```",
        )
        .expect("json");
        assert!(ok.allow);
        assert!(ok.reason.contains("workspace"));
        let deny = parse_ai_judgement("{\"allow\": false, \"reason\": \"outside workspace\"}")
            .expect("json");
        assert!(!deny.allow);
        assert!(parse_ai_judgement("not json").is_err());
        assert!(parse_ai_judgement("{\"allow\": true}").is_err());
    }
}
