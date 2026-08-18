use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::json;

use crate::ai::ApproveModel;
use crate::audit::AuditLog;
use crate::config::{ApprovalMode, ApprovalSettings};
use crate::error::PlaneError;
use crate::policy::{PolicyDecision, PolicyFile, PolicyRule};
use crate::popup::{PopupDecision, PopupSink};
use crate::request::AuthzRequest;
use crate::risk::Risk;

/// Final plane decision after policy / AI / popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
}

/// Runtime approval plane. Broker applies; this crate judges.
pub struct ApprovalPlane {
    settings: Mutex<ApprovalSettings>,
    policy: Mutex<PolicyFile>,
    policy_path: Mutex<Option<PathBuf>>,
    audit: AuditLog,
    popup: Arc<dyn PopupSink>,
    ai: Option<Arc<dyn ApproveModel>>,
}

impl ApprovalPlane {
    #[must_use]
    pub fn new(
        settings: ApprovalSettings,
        audit: AuditLog,
        popup: Arc<dyn PopupSink>,
        ai: Option<Arc<dyn ApproveModel>>,
    ) -> Self {
        Self {
            settings: Mutex::new(settings),
            policy: Mutex::new(PolicyFile::default()),
            policy_path: Mutex::new(None),
            audit,
            popup,
            ai,
        }
    }

    pub fn set_policy(&self, policy: PolicyFile) {
        *self.policy.lock() = policy;
    }

    pub fn set_policy_path(&self, path: PathBuf) {
        *self.policy_path.lock() = Some(path);
    }

    fn persist_policy(&self) -> Result<(), PlaneError> {
        let Some(path) = self.policy_path.lock().clone() else {
            return Ok(());
        };
        self.policy.lock().save_json(&path)?;
        Ok(())
    }

    pub fn set_mode(&self, mode: ApprovalMode) -> Result<(), PlaneError> {
        self.settings.lock().mode = mode;
        self.audit.append(
            "settings",
            &json!({"key": "approval.mode", "value": format!("{mode:?}")}),
        )?;
        Ok(())
    }

    #[must_use]
    pub fn mode(&self) -> ApprovalMode {
        self.settings.lock().mode
    }

    /// Judge one tool invocation. Audit write failure refuses the call.
    pub async fn authorize(&self, req: &AuthzRequest) -> Result<Decision, PlaneError> {
        let risk = Risk::classify(req);
        let mode = self.mode();
        let matched = self.policy.lock().first_match(req).cloned();
        let outcome = match mode {
            ApprovalMode::Auto => Decision::Allow,
            ApprovalMode::AskAll => {
                if risk == Risk::None && matches!(req.sensitivity, crate::Sensitivity::None) {
                    Decision::Allow
                } else {
                    self.popup(req).await?
                }
            }
            ApprovalMode::Policy => self.decide_policy(req, matched.as_ref(), risk).await?,
            ApprovalMode::AiAuto => self.decide_ai_auto(req, matched.as_ref(), risk).await?,
        };
        self.audit.append(
            "approval",
            &json!({
                "tool": req.tool,
                "target": req.target,
                "risk": format!("{risk:?}"),
                "mode": format!("{mode:?}"),
                "policy": matched.map(|rule| rule.tool),
                "decision": format!("{outcome:?}"),
            }),
        )?;
        match outcome {
            Decision::Allow => Ok(Decision::Allow),
            Decision::Deny => Err(PlaneError::Denied {
                tool: req.tool.clone(),
                reason: "denied by approval plane".to_owned(),
            }),
        }
    }

    /// Propose a rule from dialogue; popup must confirm before it is stored (P-906).
    pub async fn policy_add(
        &self,
        rule: PolicyRule,
        req_for_prompt: &AuthzRequest,
    ) -> Result<(), PlaneError> {
        match self.popup(req_for_prompt).await? {
            Decision::Allow => {}
            Decision::Deny => {
                return Err(PlaneError::Denied {
                    tool: "approval.policy_add".to_owned(),
                    reason: "policy change was not confirmed".to_owned(),
                });
            }
        }
        self.policy.lock().rules.push(rule.clone());
        self.persist_policy()?;
        self.audit.append(
            "policy",
            &json!({
                "origin": "dialogue",
                "tool": rule.tool,
                "decision": format!("{:?}", rule.decision),
            }),
        )?;
        Ok(())
    }

    #[must_use]
    pub fn policy(&self) -> PolicyFile {
        self.policy.lock().clone()
    }

    pub fn audit(&self) -> &AuditLog {
        &self.audit
    }

    async fn decide_policy(
        &self,
        req: &AuthzRequest,
        matched: Option<&PolicyRule>,
        risk: Risk,
    ) -> Result<Decision, PlaneError> {
        match matched.map(|rule| rule.decision) {
            Some(PolicyDecision::Allow) => Ok(Decision::Allow),
            Some(PolicyDecision::Deny) => Ok(Decision::Deny),
            Some(PolicyDecision::Ai) => self.ai_or_popup(req, risk).await,
            None if risk == Risk::None => Ok(Decision::Allow),
            Some(PolicyDecision::Ask) | None => self.popup(req).await,
        }
    }

    async fn decide_ai_auto(
        &self,
        req: &AuthzRequest,
        matched: Option<&PolicyRule>,
        risk: Risk,
    ) -> Result<Decision, PlaneError> {
        if let Some(rule) = matched {
            return self.decide_policy(req, Some(rule), risk).await;
        }
        self.ai_or_popup(req, risk).await
    }

    async fn ai_or_popup(&self, req: &AuthzRequest, risk: Risk) -> Result<Decision, PlaneError> {
        if risk == Risk::High {
            return self.popup(req).await;
        }
        let Some(ai) = &self.ai else {
            return self.popup(req).await;
        };
        match ai.judge(req).await {
            Ok(judgement) => {
                self.audit.append(
                    "ai_judgement",
                    &json!({
                        "tool": req.tool,
                        "allow": judgement.allow,
                        "reason": judgement.reason,
                    }),
                )?;
                if judgement.allow {
                    Ok(Decision::Allow)
                } else {
                    self.popup(req).await
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "approve model failed; falling back to popup");
                self.popup(req).await
            }
        }
    }

    async fn popup(&self, req: &AuthzRequest) -> Result<Decision, PlaneError> {
        let timeout = Duration::from_millis(self.settings.lock().popup.timeout_ms);
        let decision = self.popup.ask_timed(req, timeout).await;
        match decision {
            PopupDecision::Allow => Ok(Decision::Allow),
            PopupDecision::AllowAndRemember => {
                if req.in_workspace {
                    self.policy.lock().rules.push(PolicyRule {
                        tool: req.tool.clone(),
                        scope: Some("workspace".to_owned()),
                        decision: PolicyDecision::Allow,
                    });
                    self.persist_policy()?;
                    self.audit.append(
                        "policy",
                        &json!({
                            "origin": "remember",
                            "tool": req.tool,
                            "scope": "workspace",
                            "decision": "allow",
                        }),
                    )?;
                }
                Ok(Decision::Allow)
            }
            PopupDecision::Deny => Ok(Decision::Deny),
        }
    }
}
