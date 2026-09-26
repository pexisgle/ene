use serde::{Deserialize, Serialize};

use super::refs::{BaseViewMark, CommandWireId, ManagementTargetWire, ViewMarkWire};

pub const SETUP_SHOW_TARGET: &str = "setup:show";
pub const SETUP_COMPLETE_TARGET: &str = "setup:complete";
pub const TASK_TARGET_PREFIX: &str = "task:";
pub const WORKSPACE_TARGET_PREFIX: &str = "workspace:";
pub const USAGE_CAP_TARGET_PREFIX: &str = "cap:";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UsageCapTarget {
    pub provider: Option<String>,
    pub window: String,
    pub currency: String,
    pub limit_micros: u64,
}

#[must_use]
pub fn usage_cap_target(
    provider: Option<&str>,
    window: &str,
    currency: &str,
    limit_micros: u64,
) -> ManagementTargetWire {
    match provider {
        Some(provider) => ManagementTargetWire(format!(
            "{USAGE_CAP_TARGET_PREFIX}provider:{provider}:{window}:{currency}:{limit_micros}"
        )),
        None => ManagementTargetWire(format!(
            "{USAGE_CAP_TARGET_PREFIX}system:{window}:{currency}:{limit_micros}"
        )),
    }
}

#[must_use]
pub fn parse_usage_cap_target(target: &ManagementTargetWire) -> Option<UsageCapTarget> {
    let rest = target.0.strip_prefix(USAGE_CAP_TARGET_PREFIX)?;
    let mut parts = rest.split(':');
    let provider = match parts.next()? {
        "system" => None,
        "provider" => Some(non_empty(parts.next()?)?),
        _ => return None,
    };
    let window = non_empty(parts.next()?)?;
    let currency = non_empty(parts.next()?)?;
    let limit_micros = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(UsageCapTarget {
        provider,
        window,
        currency,
        limit_micros,
    })
}

fn non_empty(part: &str) -> Option<String> {
    if part.is_empty() {
        return None;
    }
    Some(part.to_owned())
}

#[must_use]
pub fn workspace_target(path: &str) -> ManagementTargetWire {
    ManagementTargetWire(format!("{WORKSPACE_TARGET_PREFIX}{path}"))
}

#[must_use]
pub fn parse_workspace_target(target: &ManagementTargetWire) -> Option<&str> {
    let rest = target.0.strip_prefix(WORKSPACE_TARGET_PREFIX)?;
    if rest.is_empty() {
        return None;
    }
    Some(rest)
}

#[must_use]
pub fn task_target(task: uuid::Uuid) -> ManagementTargetWire {
    ManagementTargetWire(format!("{TASK_TARGET_PREFIX}{}", task.as_hyphenated()))
}

#[must_use]
pub fn parse_task_target(target: &ManagementTargetWire) -> Option<uuid::Uuid> {
    let rest = target.0.strip_prefix(TASK_TARGET_PREFIX)?;
    uuid::Uuid::parse_str(rest).ok()
}

#[must_use]
pub fn credential_target(provider: &str, label: &str) -> ManagementTargetWire {
    ManagementTargetWire(format!("credential:{provider}:{label}"))
}

#[must_use]
pub fn consent_target(
    capability: &str,
    provider: &str,
    model: &str,
    credential_id: &str,
) -> ManagementTargetWire {
    ManagementTargetWire(format!(
        "consent:{capability}:{provider}:{model}:{credential_id}"
    ))
}

#[must_use]
pub fn parse_credential_target(target: &ManagementTargetWire) -> Option<(String, String)> {
    let rest = target.0.strip_prefix("credential:")?;
    let (provider, label) = rest.split_once(':')?;
    if provider.is_empty() || label.is_empty() {
        return None;
    }
    Some((provider.to_owned(), label.to_owned()))
}

#[must_use]
pub fn parse_consent_target(
    target: &ManagementTargetWire,
) -> Option<(String, String, String, String)> {
    let rest = target.0.strip_prefix("consent:")?;
    let mut parts = rest.splitn(4, ':');
    let capability = parts.next()?;
    let provider = parts.next()?;
    let model = parts.next()?;
    let credential_id = parts.next()?;
    if capability.is_empty() || provider.is_empty() || model.is_empty() || credential_id.is_empty()
    {
        return None;
    }
    Some((
        capability.to_owned(),
        provider.to_owned(),
        model.to_owned(),
        credential_id.to_owned(),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ManagementIntentKind {
    StopCompanion,
    DeleteCompanion,
    CancelTask,
    ResumeTask,
    SelectWorkspace,
    ManageSchedule,
    DenyOrRefuse,
    ManageRuleConsentCap,
    ManageDevice,
    ConfigureCredentialIntent,
    RequestDeletionBackupRestoreReset,
}

impl ManagementIntentKind {
    #[must_use]
    pub fn target_carries_owner_body(self) -> bool {
        matches!(self, Self::RequestDeletionBackupRestoreReset)
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagementIntent {
    pub intent_id: CommandWireId,
    pub kind: ManagementIntentKind,
    pub target: ManagementTargetWire,
    pub base_view: BaseViewMark,
    pub rationale: IntentRationaleWire,
    #[serde(default)]
    pub confirmed: bool,
}

impl ManagementIntent {
    #[must_use]
    pub fn target_carries_owner_body(&self) -> bool {
        self.kind.target_carries_owner_body()
            || self
                .target
                .0
                .starts_with(super::deletion::DELETION_TARGET_PREFIX)
    }
}

impl core::fmt::Debug for ManagementIntent {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut builder = formatter.debug_struct("ManagementIntent");
        builder
            .field("intent_id", &self.intent_id)
            .field("kind", &self.kind);
        if self.target_carries_owner_body() {
            builder.field("target", &"[redacted]");
        } else {
            builder.field("target", &self.target);
        }
        builder
            .field("base_view", &self.base_view)
            .field("rationale", &"[redacted]")
            .field("confirmed", &self.confirmed)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentRationaleWire {
    pub origin: RationaleOrigin,
    pub quote: Option<String>,
}

impl core::fmt::Debug for IntentRationaleWire {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("IntentRationaleWire")
            .field("origin", &self.origin)
            .field("quote", &self.quote.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum RationaleOrigin {
    Conversation,
    ManagementSurface,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ManagementOutcome {
    AppliedAsOneTime,
    StoredAsRuleView { revision: ViewMarkWire },
    NeedsClarification,
    DeniedByBoundary,
    StaleBaseView { current: ViewMarkWire },
    HeldByOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagementViewRequest {
    pub sections: Vec<String>,
    pub memory_after: Option<String>,
    pub memory_revisions_of: Option<String>,
    pub memory_revisions_after: Option<u64>,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewSection {
    pub kind: String,
    pub title: String,
    pub body: String,
}

impl core::fmt::Debug for ViewSection {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ViewSection")
            .field("kind", &self.kind)
            .field("title", &self.title)
            .field("body", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagementView {
    pub mark: ViewMarkWire,
    pub sections: Vec<ViewSection>,
}

#[cfg(test)]
mod tests {
    use super::super::refs::{BaseViewMark, CommandWireId, ManagementTargetWire};
    use super::ViewSection;
    use super::{
        IntentRationaleWire, ManagementIntent, ManagementIntentKind, RationaleOrigin,
        SETUP_COMPLETE_TARGET, SETUP_SHOW_TARGET, UsageCapTarget, consent_target,
        credential_target, parse_consent_target, parse_credential_target, parse_task_target,
        parse_usage_cap_target, parse_workspace_target, task_target, usage_cap_target,
        workspace_target,
    };
    use uuid::Uuid;

    fn intent() -> ManagementIntent {
        ManagementIntent {
            intent_id: CommandWireId(Uuid::new_v4()),
            kind: ManagementIntentKind::ManageSchedule,
            target: ManagementTargetWire(String::from("schedule-1")),
            base_view: BaseViewMark(String::from("mark-1")),
            rationale: IntentRationaleWire {
                origin: RationaleOrigin::ManagementSurface,
                quote: Some(String::from("quoted private words")),
            },
            confirmed: false,
        }
    }

    #[test]
    fn management_intent_debug_redacts_rationale_and_owner_targets() {
        let rendered = format!("{:?}", intent());
        assert!(!rendered.contains("quoted private words"));
        assert!(rendered.contains("schedule-1"));
        assert!(rendered.contains("mark-1"));
        assert!(rendered.contains("confirmed"));

        let mut deletion = intent();
        deletion.kind = ManagementIntentKind::RequestDeletionBackupRestoreReset;
        deletion.target = ManagementTargetWire(String::from("deletion:privacy:raw secret body"));
        let rendered = format!("{deletion:?}");
        assert!(!rendered.contains("raw secret body"));
        assert!(
            rendered.contains("deletion-backup-restore-reset")
                || rendered.contains("RequestDeletion")
        );
        assert!(
            ManagementIntentKind::RequestDeletionBackupRestoreReset.target_carries_owner_body()
        );
        assert!(!ManagementIntentKind::ManageSchedule.target_carries_owner_body());

        let mut mismatched = intent();
        mismatched.kind = ManagementIntentKind::ManageSchedule;
        mismatched.target = ManagementTargetWire(String::from("deletion:privacy:raw secret body"));
        let rendered = format!("{mismatched:?}");
        assert!(!rendered.contains("raw secret body"));
        assert!(mismatched.target_carries_owner_body());

        let section = ViewSection {
            kind: String::from("setup"),
            title: String::from("Setup status"),
            body: String::from("quoted managed content"),
        };
        let rendered = format!("{section:?}");
        assert!(!rendered.contains("quoted managed content"));
        assert!(rendered.contains("Setup status"));
    }

    #[test]
    fn omitted_confirmed_deserializes_false_and_true_is_never_host_confirmation() {
        let baseline = intent();
        let json = serde_json::to_value(&baseline).expect("intent serializes");
        let serde_json::Value::Object(mut map) = json else {
            panic!("intent JSON must be an object");
        };
        map.remove("confirmed");
        let omitted: ManagementIntent =
            serde_json::from_value(serde_json::Value::Object(map)).expect("omitted confirmed");
        assert!(!omitted.confirmed);
        let mut declared = intent();
        declared.confirmed = true;
        let back: ManagementIntent =
            serde_json::from_str(&serde_json::to_string(&declared).expect("serializes"))
                .expect("roundtrip");
        assert!(back.confirmed);
    }

    #[test]
    fn management_target_grammars_roundtrip_and_reject_ambiguous_shapes() {
        assert_eq!(SETUP_SHOW_TARGET, "setup:show");
        assert_eq!(SETUP_COMPLETE_TARGET, "setup:complete");

        let credential = credential_target("openai", "personal");
        assert_eq!(credential.0.as_str(), "credential:openai:personal");
        assert_eq!(
            parse_credential_target(&credential),
            Some((String::from("openai"), String::from("personal")))
        );
        let dialogue = consent_target("dialogue", "openai", "gpt-x", "cred-1");
        assert_eq!(dialogue.0.as_str(), "consent:dialogue:openai:gpt-x:cred-1");
        let learning = consent_target("learning", "openai", "gpt-x", "cred-1");
        assert_eq!(learning.0.as_str(), "consent:learning:openai:gpt-x:cred-1");
        assert_eq!(
            parse_consent_target(&learning),
            Some((
                String::from("learning"),
                String::from("openai"),
                String::from("gpt-x"),
                String::from("cred-1")
            ))
        );
        let colon_id = consent_target("dialogue", "openai", "gpt-x", "cred:with:colons");
        assert_eq!(
            colon_id.0.as_str(),
            "consent:dialogue:openai:gpt-x:cred:with:colons"
        );
        assert_eq!(
            parse_consent_target(&colon_id),
            Some((
                String::from("dialogue"),
                String::from("openai"),
                String::from("gpt-x"),
                String::from("cred:with:colons")
            ))
        );

        for raw in [
            "credential::personal",
            "credential:openai:",
            "credential::",
            "credential:openai",
            "credential:",
            "consent:dialogue:openai:gpt-x:cred-1",
            "setup:show",
            "setup:complete",
            "",
        ] {
            assert!(parse_credential_target(&ManagementTargetWire(String::from(raw))).is_none());
        }
        for raw in [
            "consent::openai:gpt-x:cred-1",
            "consent:dialogue::gpt-x:cred-1",
            "consent:dialogue:openai::cred-1",
            "consent:dialogue:openai:gpt-x:",
            "consent:dialogue:openai:gpt-x",
            "consent:dialogue:openai",
            "consent:dialogue",
            "consent:",
            "consent:openai:gpt-x:cred-1",
            "credential:openai:personal",
            "setup:show",
            "setup:complete",
            "",
        ] {
            assert!(parse_consent_target(&ManagementTargetWire(String::from(raw))).is_none());
        }

        let id = uuid::Uuid::new_v4();
        assert_eq!(parse_task_target(&task_target(id)), Some(id));
        for raw in [
            "task:not-a-uuid",
            "task:",
            "credential:openai:personal",
            "setup:show",
            "",
        ] {
            assert!(parse_task_target(&ManagementTargetWire(String::from(raw))).is_none());
        }

        let workspace = workspace_target("/srv/workspace/ene");
        assert_eq!(
            parse_workspace_target(&workspace),
            Some("/srv/workspace/ene")
        );
        let windows = workspace_target("C:\\Users\\ene\\workspace");
        assert_eq!(
            parse_workspace_target(&windows),
            Some("C:\\Users\\ene\\workspace")
        );
        for raw in ["workspace:", "task:abc", "setup:show", ""] {
            assert!(parse_workspace_target(&ManagementTargetWire(String::from(raw))).is_none());
        }
    }

    #[test]
    fn usage_cap_target_grammar_preserves_scope_and_never_guesses() {
        let system = usage_cap_target(None, "daily_utc", "USD", 1_000_000);
        assert_eq!(system.0.as_str(), "cap:system:daily_utc:USD:1000000");
        assert_eq!(
            parse_usage_cap_target(&system),
            Some(UsageCapTarget {
                provider: None,
                window: String::from("daily_utc"),
                currency: String::from("USD"),
                limit_micros: 1_000_000,
            })
        );
        let provider = usage_cap_target(Some("openai"), "monthly_utc", "USD", 42);
        assert_eq!(
            provider.0.as_str(),
            "cap:provider:openai:monthly_utc:USD:42"
        );
        assert_eq!(
            parse_usage_cap_target(&provider),
            Some(UsageCapTarget {
                provider: Some(String::from("openai")),
                window: String::from("monthly_utc"),
                currency: String::from("USD"),
                limit_micros: 42,
            })
        );
        for raw in [
            "cap:",
            "cap:system",
            "cap:system:daily_utc",
            "cap:system:daily_utc:USD",
            "cap:system:daily_utc:USD:not-a-number",
            "cap:system:daily_utc:USD:-1",
            "cap:system:daily_utc:USD:1:extra",
            "cap:provider:openai:daily_utc:USD",
            "cap:provider::daily_utc:USD:1",
            "cap:provider:openai::USD:1",
            "cap:provider:openai:daily_utc::1",
            "cap:global:daily_utc:USD:1",
            "consent:dialogue:openai:gpt-x:cred-1",
            "setup:show",
            "",
        ] {
            assert_eq!(
                parse_usage_cap_target(&ManagementTargetWire(String::from(raw))),
                None
            );
        }
        assert!(parse_usage_cap_target(&usage_cap_target(None, "daily_utc", "USD", 0)).is_some());
    }
}
