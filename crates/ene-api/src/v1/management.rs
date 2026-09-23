use serde::{Deserialize, Serialize};

use super::refs::{BaseViewMark, CommandWireId, ManagementTargetWire, ViewMarkWire};

pub const SETUP_SHOW_TARGET: &str = "setup:show";
pub const SETUP_COMPLETE_TARGET: &str = "setup:complete";
pub const TASK_TARGET_PREFIX: &str = "task:";
pub const WORKSPACE_TARGET_PREFIX: &str = "workspace:";
pub const USAGE_CAP_TARGET_PREFIX: &str = "cap:";

/// Parsed usage-cap target: exactly the assignment parameters. The scope is
/// carried by [`Self::provider`] alone (`None` is the system scope), so an
/// inconsistent `(scope, provider)` pair cannot exist.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UsageCapTarget {
    /// Provider name for the provider scope, `None` for the system scope.
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

/// Exact rule: strip the `cap:` prefix and parse the two shapes of
/// [`USAGE_CAP_TARGET_PREFIX`]. The first token fixes the shape, every part
/// must be non-empty, and the limit must be a plain decimal `u64`; anything
/// else is `None`, never a guessed cap. The closed window/currency
/// vocabularies are validated by the owner at command time, so the grammar
/// stays vocabulary-neutral.
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

/// Management intent kinds (IPC §18.2). The kind name never decides the
/// trust class: the Host classifies by operation, target, and impact, and
/// each owner alone may accept its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// Whether this intent's target carries the Owner's deletion body,
    /// independent of the client-declared `kind`: the wire fields are
    /// independent, so the target grammar — never the self-declared kind —
    /// decides whether the body may be rendered or journaled.
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
pub enum RationaleOrigin {
    Conversation,
    ManagementSurface,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ManagementOutcome {
    AppliedAsOneTime,
    StoredAsRuleView { revision: ViewMarkWire },
    NeedsClarification,
    DeniedByBoundary,
    StaleBaseView { current: ViewMarkWire },
    HeldByOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ManagementViewRequest {
    pub sections: Vec<String>,
    /// Current Memory id the `memory` section continues after, exclusive.
    ///
    /// The Host renders at most one page of the memory section and ends it
    /// with a `next: <id>` line while older memories remain; passing that id
    /// back here reads the next page. `None` starts at the newest. The field
    /// is the typed read query for the one paged section, never a query
    /// syntax embedded in a section name.
    pub memory_after: Option<String>,
    /// When set, the `memory` section renders one Memory's revision history
    /// (with grounds) instead of the current list. The value is the Memory id
    /// from the list. The revision history is paged independently, so it is
    /// never inflated into the list page.
    pub memory_revisions_of: Option<String>,
    /// Revision number the revision page continues after, exclusive, oldest
    /// first. Semantics mirror [`memory_after`](Self::memory_after); the Host
    /// ends the page with a `next-revision: <n>` line while newer revisions
    /// remain. `None` or zero starts at the first revision.
    pub memory_revisions_after: Option<u64>,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
pub struct ManagementView {
    pub mark: ViewMarkWire,
    pub sections: Vec<ViewSection>,
}

#[cfg(test)]
mod tests {
    use super::super::refs::{BaseViewMark, CommandWireId, ManagementTargetWire, ViewMarkWire};
    use super::{
        IntentRationaleWire, ManagementIntent, ManagementIntentKind, RationaleOrigin,
        SETUP_COMPLETE_TARGET, SETUP_SHOW_TARGET, UsageCapTarget, consent_target,
        credential_target, parse_consent_target, parse_credential_target, parse_task_target,
        parse_usage_cap_target, parse_workspace_target, task_target, usage_cap_target,
        workspace_target,
    };
    use super::{ManagementOutcome, ViewSection};
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
    fn intent_debug_redacts_rationale() {
        let rendered = format!("{:?}", intent());
        assert!(!rendered.contains("quoted private words"));
        assert!(rendered.contains("schedule-1"));
        assert!(rendered.contains("mark-1"));
        assert!(rendered.contains("confirmed"));
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
    fn deletion_intent_debug_redacts_the_owner_body_target() {
        let mut intent = intent();
        intent.kind = ManagementIntentKind::RequestDeletionBackupRestoreReset;
        intent.target = ManagementTargetWire(String::from("deletion:privacy:raw secret body"));
        let rendered = format!("{intent:?}");
        assert!(!rendered.contains("raw secret body"));
        assert!(
            rendered.contains("deletion-backup-restore-reset")
                || rendered.contains("RequestDeletion")
        );
        assert!(
            ManagementIntentKind::RequestDeletionBackupRestoreReset.target_carries_owner_body()
        );
        assert!(!ManagementIntentKind::ManageSchedule.target_carries_owner_body());
    }

    #[test]
    fn deletion_body_is_redacted_even_under_a_mismatched_kind() {
        let mut intent = intent();
        intent.kind = ManagementIntentKind::ManageSchedule;
        intent.target = ManagementTargetWire(String::from("deletion:privacy:raw secret body"));
        let rendered = format!("{intent:?}");
        assert!(
            !rendered.contains("raw secret body"),
            "the target grammar, not the declared kind, decides redaction: {rendered}"
        );
        assert!(
            intent.target_carries_owner_body(),
            "a deletion target carries an Owner body whatever the kind claims"
        );
    }

    #[test]
    fn stale_base_view_points_at_the_current_mark() {
        let outcome = ManagementOutcome::StaleBaseView {
            current: ViewMarkWire(String::from("mark-2")),
        };
        let rendered = format!("{outcome:?}");
        assert!(
            rendered.contains("mark-2"),
            "current mark stays visible: {rendered}"
        );
    }

    #[test]
    fn section_debug_redacts_body() {
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
    fn credential_builder_spells_the_shared_grammar() {
        let target = credential_target("openai", "personal");
        assert_eq!(target.0.as_str(), "credential:openai:personal");
    }

    #[test]
    fn consent_builder_spells_the_shared_grammar() {
        let target = consent_target("dialogue", "openai", "gpt-x", "cred-1");
        assert_eq!(target.0.as_str(), "consent:dialogue:openai:gpt-x:cred-1");
        let learning = consent_target("learning", "openai", "gpt-x", "cred-1");
        assert_eq!(learning.0.as_str(), "consent:learning:openai:gpt-x:cred-1");
    }

    #[test]
    fn credential_builder_parser_roundtrip() {
        let target = credential_target("openai", "personal");
        assert_eq!(
            parse_credential_target(&target),
            Some((String::from("openai"), String::from("personal")))
        );
    }

    #[test]
    fn consent_builder_parser_roundtrip() {
        let target = consent_target("learning", "openai", "gpt-x", "cred-1");
        assert_eq!(
            parse_consent_target(&target),
            Some((
                String::from("learning"),
                String::from("openai"),
                String::from("gpt-x"),
                String::from("cred-1")
            ))
        );
    }

    #[test]
    fn credential_parser_rejects_blanks_and_wrong_shapes() {
        for raw in [
            "credential::personal",
            "credential:openai:",
            "credential::",
            "credential:openai",
            "credential:",
            "consent:dialogue:openai:gpt-x:cred-1",
            "setup:show",
            "",
        ] {
            let target = ManagementTargetWire(String::from(raw));
            assert!(parse_credential_target(&target).is_none());
        }
    }

    #[test]
    fn consent_parser_rejects_blanks_and_wrong_shapes() {
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
            "setup:complete",
            "",
        ] {
            let target = ManagementTargetWire(String::from(raw));
            assert!(parse_consent_target(&target).is_none());
        }
    }

    #[test]
    fn consent_parser_preserves_colons_in_credential_id() {
        let target = consent_target("dialogue", "openai", "gpt-x", "cred:with:colons");
        assert_eq!(
            target.0.as_str(),
            "consent:dialogue:openai:gpt-x:cred:with:colons"
        );
        assert_eq!(
            parse_consent_target(&target),
            Some((
                String::from("dialogue"),
                String::from("openai"),
                String::from("gpt-x"),
                String::from("cred:with:colons")
            ))
        );
    }

    #[test]
    fn setup_command_targets_are_fixed_strings() {
        assert_eq!(SETUP_SHOW_TARGET, "setup:show");
        assert_eq!(SETUP_COMPLETE_TARGET, "setup:complete");
    }

    #[test]
    fn task_target_roundtrips_and_rejects_other_text() {
        let id = uuid::Uuid::new_v4();
        let target = task_target(id);
        assert_eq!(parse_task_target(&target), Some(id));
        for raw in [
            "task:not-a-uuid",
            "task:",
            "credential:openai:personal",
            "setup:show",
            "",
        ] {
            assert_eq!(
                parse_task_target(&ManagementTargetWire(String::from(raw))),
                None
            );
        }
    }

    #[test]
    fn workspace_target_roundtrips_with_colons_and_rejects_empty() {
        let target = workspace_target("/srv/workspace/ene");
        assert_eq!(parse_workspace_target(&target), Some("/srv/workspace/ene"));
        let windows = workspace_target("C:\\Users\\ene\\workspace");
        assert_eq!(
            parse_workspace_target(&windows),
            Some("C:\\Users\\ene\\workspace")
        );
        for raw in ["workspace:", "task:abc", "setup:show", ""] {
            assert_eq!(
                parse_workspace_target(&ManagementTargetWire(String::from(raw))),
                None
            );
        }
    }

    #[test]
    fn setup_command_targets_parse_as_neither_shape() {
        for raw in [SETUP_SHOW_TARGET, SETUP_COMPLETE_TARGET] {
            let target = ManagementTargetWire(String::from(raw));
            assert!(
                parse_credential_target(&target).is_none(),
                "setup target is not a credential target: {raw:?}"
            );
            assert!(
                parse_consent_target(&target).is_none(),
                "setup target is not a consent target: {raw:?}"
            );
        }
    }

    #[test]
    fn usage_cap_target_roundtrips_both_scopes() {
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
    }

    #[test]
    fn usage_cap_target_rejects_other_shapes_and_never_guesses() {
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
        // Zero is representable text; the owner refuses it as a limit, so the
        // grammar does not pre-decide that domain outcome.
        assert!(parse_usage_cap_target(&usage_cap_target(None, "daily_utc", "USD", 0)).is_some());
    }
}
