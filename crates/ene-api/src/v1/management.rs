//! Setup management inlet: intent in, filtered view out (IPC §18).
//!
//! The Client expresses intent and reads filtered views; every acceptance,
//! confirmation, and high-privilege final check happens Host-side.
//! High-privilege final confirmation additionally never travels this wire:
//! it stays on the Host-local trusted first-party surface (IPC §18.1).
//!
//! Setup target grammar: the single shared contract for Setup management
//! targets. The Host and the CLI must both use it; neither side re-invents
//! the mini-language.
//!
//! ```text
//! credential-target = "credential:" provider ":" label
//! capability        = "dialogue" | "learning"
//! consent-target    = "consent:" capability ":" provider ":" model ":" credential-id
//! setup-show        = "setup:show"
//! setup-complete    = "setup:complete"
//! ```
//!
//! Builders ([`credential_target`], [`consent_target`]) and parsers
//! ([`parse_credential_target`], [`parse_consent_target`]) are two sides of
//! this one contract. The Host parse is authoritative: builders are plain
//! constructors that never bypass validation, and every part must be
//! non-empty at parse time. The consent `credential-id` and the credential
//! `label` keep further `':'` characters verbatim. The two Setup command
//! targets are fixed strings ([`SETUP_SHOW_TARGET`],
//! [`SETUP_COMPLETE_TARGET`]).

use serde::{Deserialize, Serialize};

use super::refs::{BaseViewMark, CommandWireId, ManagementTargetWire, ViewMarkWire};

/// Fixed Setup command target requesting the current Setup view.
pub const SETUP_SHOW_TARGET: &str = "setup:show";

/// Fixed Setup command target marking Setup complete.
pub const SETUP_COMPLETE_TARGET: &str = "setup:complete";

/// Fixed prefix of one Task-targeting management intent: `task:` plus the
/// Task identity. The grammar is shared Host-side so a Client or CLI builds
/// exactly what the Host parses.
pub const TASK_TARGET_PREFIX: &str = "task:";

/// Fixed prefix of one Workspace-targeting first-party management intent:
/// `workspace:` plus the Owner-selected absolute folder path. The remainder is
/// kept verbatim (paths may contain `:`), and the Host validates it
/// canonically before it becomes a trusted premise.
pub const WORKSPACE_TARGET_PREFIX: &str = "workspace:";

/// Fixed prefix of one usage-cap management intent
/// (`usage-cost-cap` §13/§17):
///
/// ```text
/// usage-cap = "cap:" ( "system:" window ":" currency ":" limit-micros
///                    | "provider:" provider ":" window ":" currency ":" limit-micros )
/// window    = "daily_utc" | "monthly_utc"
/// currency  = currency code (the owner vocabulary, e.g. "USD")
/// ```
///
/// The target carries the intended limit only; it is never authority. The
/// intent's `base_view` is the opaque cap mark the reader saw, and the Host
/// re-checks it against the current revision before the permission-owned
/// command runs.
pub const USAGE_CAP_TARGET_PREFIX: &str = "cap:";

/// Parsed usage-cap target: exactly the assignment parameters.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UsageCapTarget {
    /// `"system"` or `"provider"`.
    pub scope: String,
    /// Provider name for the provider scope, `None` for the system scope.
    pub provider: Option<String>,
    /// `"daily_utc"` or `"monthly_utc"`.
    pub window: String,
    /// Currency code, as the owner's closed vocabulary spells it.
    pub currency: String,
    /// Exact limit in micro-currency units. Zero is a representable target
    /// that the owner refuses as `InvalidLimit`; the grammar never decides.
    pub limit_micros: u64,
}

/// Plain constructor: it does not validate. Non-empty parts and the closed
/// scope/window vocabularies are enforced at Host parse, which stays
/// authoritative.
#[must_use]
pub fn usage_cap_target(
    scope: &str,
    provider: Option<&str>,
    window: &str,
    currency: &str,
    limit_micros: u64,
) -> ManagementTargetWire {
    match provider {
        Some(provider) => ManagementTargetWire(format!(
            "{USAGE_CAP_TARGET_PREFIX}{scope}:{provider}:{window}:{currency}:{limit_micros}"
        )),
        None => ManagementTargetWire(format!(
            "{USAGE_CAP_TARGET_PREFIX}{scope}:{window}:{currency}:{limit_micros}"
        )),
    }
}

/// Exact rule: strip the `cap:` prefix and parse the two shapes of
/// [`USAGE_CAP_TARGET_PREFIX`]. Every part must be non-empty and the limit
/// must be a plain decimal `u64`; anything else is `None`, never a guessed
/// cap. The closed scope/window/currency vocabularies are validated by the
/// owner at command time, so the grammar stays vocabulary-neutral.
#[must_use]
pub fn parse_usage_cap_target(target: &ManagementTargetWire) -> Option<UsageCapTarget> {
    let rest = target.0.strip_prefix(USAGE_CAP_TARGET_PREFIX)?;
    let mut parts = rest.split(':');
    let scope = parts.next()?;
    match scope {
        "system" => {
            let window = non_empty(parts.next()?)?;
            let currency = non_empty(parts.next()?)?;
            let limit_micros = parts.next()?.parse::<u64>().ok()?;
            if parts.next().is_some() {
                return None;
            }
            Some(UsageCapTarget {
                scope: String::from("system"),
                provider: None,
                window,
                currency,
                limit_micros,
            })
        }
        "provider" => {
            let provider = non_empty(parts.next()?)?;
            let window = non_empty(parts.next()?)?;
            let currency = non_empty(parts.next()?)?;
            let limit_micros = parts.next()?.parse::<u64>().ok()?;
            if parts.next().is_some() {
                return None;
            }
            Some(UsageCapTarget {
                scope: String::from("provider"),
                provider: Some(provider),
                window,
                currency,
                limit_micros,
            })
        }
        _ => None,
    }
}

fn non_empty(part: &str) -> Option<String> {
    if part.is_empty() {
        return None;
    }
    Some(part.to_owned())
}

/// Plain constructor: it does not validate. The Host parse and validation
/// stay authoritative.
#[must_use]
pub fn workspace_target(path: &str) -> ManagementTargetWire {
    ManagementTargetWire(format!("{WORKSPACE_TARGET_PREFIX}{path}"))
}

/// Exact rule: strip the `workspace:` prefix and require a non-empty
/// remainder, else [`None`]. The remainder is the Owner-authored path
/// verbatim; validation happens Host-side.
#[must_use]
pub fn parse_workspace_target(target: &ManagementTargetWire) -> Option<&str> {
    let rest = target.0.strip_prefix(WORKSPACE_TARGET_PREFIX)?;
    if rest.is_empty() {
        return None;
    }
    Some(rest)
}

/// Plain constructor: it does not validate. The Host parse stays
/// authoritative.
#[must_use]
pub fn task_target(task: uuid::Uuid) -> ManagementTargetWire {
    ManagementTargetWire(format!("{TASK_TARGET_PREFIX}{}", task.as_hyphenated()))
}

/// Exact rule: strip the `task:` prefix and require the remainder to parse as
/// a UUID, else [`None`]. No other text is a Task target.
#[must_use]
pub fn parse_task_target(target: &ManagementTargetWire) -> Option<uuid::Uuid> {
    let rest = target.0.strip_prefix(TASK_TARGET_PREFIX)?;
    uuid::Uuid::parse_str(rest).ok()
}

/// Plain constructor: it does not validate. Non-empty `provider` and
/// `label` are enforced at Host parse, which stays authoritative.
#[must_use]
pub fn credential_target(provider: &str, label: &str) -> ManagementTargetWire {
    ManagementTargetWire(format!("credential:{provider}:{label}"))
}

/// Plain constructor: it does not validate. Non-empty `capability`,
/// `provider`, `model`, and `credential-id` are enforced at Host parse, which
/// stays authoritative.
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

/// Exact rule: strip the `credential:` prefix, split the remainder once on
/// `':'`, and require both parts non-empty, else [`None`]. The label keeps
/// any further `':'` verbatim.
#[must_use]
pub fn parse_credential_target(target: &ManagementTargetWire) -> Option<(String, String)> {
    let rest = target.0.strip_prefix("credential:")?;
    let (provider, label) = rest.split_once(':')?;
    if provider.is_empty() || label.is_empty() {
        return None;
    }
    Some((provider.to_owned(), label.to_owned()))
}

/// Exact rule: strip the `consent:` prefix, split the remainder with
/// `splitn(4, ':')`, and require all four parts non-empty, else [`None`].
/// The `credential-id` keeps its remainder verbatim, so it may itself
/// contain `':'`.
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

/// Management intent kinds (IPC §18.2). Stage 1 exercises the Setup range;
/// the remaining kinds arrive with their owners, which alone may accept
/// them. The kind name never decides the trust class: the Host classifies
/// by operation, target, and impact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ManagementIntentKind {
    StopCompanion,
    DeleteCompanion,
    CancelTask,
    /// Resume one interrupted Task explicitly (H-A.1 / AU17). The target is
    /// `task:{task-id}` and the rationale quote carries the Owner's resume
    /// instruction body; the Host records the first-party activity, composes
    /// the premise from durable state, and runs the same owner gate as the
    /// conversation path. No presence or provider success is required, and
    /// an offline opener never starts a runner.
    ResumeTask,
    /// Select the Owner-confirmed Workspace folder for Task work. This is the
    /// trusted first-party premise a Task association may use; provider
    /// output never carries one.
    SelectWorkspace,
    ManageSchedule,
    /// Deny or refuse rule/consent handling.
    DenyOrRefuse,
    /// Rule / consent / cap management. The consent grammar
    /// (`consent:{capability}:...`) assigns a route; the cap grammar
    /// (`cap:{scope}:{window}:{currency}:{limit}`, `usage-cost-cap` §13)
    /// sets a usage cap, whose currentness is the intent `base_view` mark.
    ManageRuleConsentCap,
    ManageDevice,
    /// Credential configuration intent (values travel the protected
    /// Host-local path only, never this payload).
    ConfigureCredentialIntent,
    /// Targeted Deletion request inlet (Stage 6 A1b; lifecycle §4, §15). The
    /// target grammar is `deletion:{purpose}:{exact-text}` (see
    /// [`super::deletion`]), the exact text is the Owner body, and the intent
    /// only ever stages a request: the destructive final confirmation is
    /// established on the Host-local trusted first-party surface (IPC §18.1)
    /// and never travels this payload. The same kind also names the deferred
    /// backup / restore / reset families, which have no producer yet and
    /// clarify instead of borrowing this one's authority.
    RequestDeletionBackupRestoreReset,
}

impl ManagementIntentKind {
    /// Whether this kind's target grammar may carry an Owner body.
    ///
    /// Only the Targeted Deletion target does (`deletion:{purpose}:{exact
    /// text}`): [`ManagementIntent`]'s `Debug` redacts that target so no log
    /// or panic message reproduces the Owner's text. The kind names the
    /// grammar, never a trust class.
    #[must_use]
    pub fn target_carries_owner_body(self) -> bool {
        matches!(self, Self::RequestDeletionBackupRestoreReset)
    }
}

/// One management intent: idempotent by key, advisory by nature. The Host
/// maps it to a domain premise and answers with [`ManagementOutcome`];
/// the Client never self-declares the result.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ManagementIntent {
    pub intent_id: CommandWireId,
    pub kind: ManagementIntentKind,
    pub target: ManagementTargetWire,
    /// Display-revision mark the intent was built on. Staleness is checked,
    /// never defaulted to unconstrained.
    pub base_view: BaseViewMark,
    pub rationale: IntentRationaleWire,
}

impl core::fmt::Debug for ManagementIntent {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut builder = formatter.debug_struct("ManagementIntent");
        builder
            .field("intent_id", &self.intent_id)
            .field("kind", &self.kind);
        if self.kind.target_carries_owner_body() {
            builder.field("target", &"[redacted]");
        } else {
            builder.field("target", &self.target);
        }
        builder
            .field("base_view", &self.base_view)
            .field("rationale", &"[redacted]")
            .finish()
    }
}

/// Owner intent record: where the intent came from plus the quoted
/// correspondence. The quote may reproduce managed content, so it is
/// redacted from [`core::fmt::Debug`].
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

/// Management outcome: an Ok-side domain outcome, never an error (IPC
/// §18.2, §24).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ManagementOutcome {
    /// Applied as a one-time approval. Never a standing rule.
    AppliedAsOneTime,
    /// Stored as a rule or similar, with its revision view.
    StoredAsRuleView { revision: ViewMarkWire },
    /// Too ambiguous, contradictory, excessive, or grave to decide.
    NeedsClarification,
    /// Silent control-boundary overwrite or trusted-surface violation.
    DeniedByBoundary,
    /// The base view moved underneath the intent.
    StaleBaseView {
        /// Current mark the sender should build on next time.
        current: ViewMarkWire,
    },
    /// Held by deletion, restore, stop, or similar prohibitions.
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
    #[serde(default)]
    pub memory_after: Option<String>,
    /// When set, the `memory` section renders one Memory's revision history
    /// (with grounds) instead of the current list. The value is the Memory id
    /// from the list. The revision history is paged independently, so it is
    /// never inflated into the list page.
    #[serde(default)]
    pub memory_revisions_of: Option<String>,
    /// Revision number the revision page continues after, exclusive, oldest
    /// first. Semantics mirror [`memory_after`](Self::memory_after); the Host
    /// ends the page with a `next-revision: <n>` line while newer revisions
    /// remain. `None` or zero starts at the first revision.
    #[serde(default)]
    pub memory_revisions_after: Option<u64>,
}

/// Short labels stay visible; bodies redact.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ViewSection {
    pub kind: String,
    pub title: String,
    /// May quote managed content; redacted from [`core::fmt::Debug`].
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

/// Secrets, judgment copies, and full internal conditions are never included.
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
        }
    }

    #[test]
    fn intent_debug_redacts_rationale() {
        let rendered = format!("{:?}", intent());
        assert!(
            !rendered.contains("quoted private words"),
            "rationale redacted: {rendered}"
        );
        assert!(
            rendered.contains("schedule-1"),
            "refs stay visible: {rendered}"
        );
        assert!(
            rendered.contains("mark-1"),
            "marks stay visible: {rendered}"
        );
    }

    #[test]
    fn deletion_intent_debug_redacts_the_owner_body_target() {
        let mut intent = intent();
        intent.kind = ManagementIntentKind::RequestDeletionBackupRestoreReset;
        intent.target = ManagementTargetWire(String::from("deletion:privacy:raw secret body"));
        let rendered = format!("{intent:?}");
        assert!(
            !rendered.contains("raw secret body"),
            "the deletion target body is redacted: {rendered}"
        );
        assert!(
            rendered.contains("deletion-backup-restore-reset")
                || rendered.contains("RequestDeletion"),
            "the kind stays visible: {rendered}"
        );
        assert!(
            ManagementIntentKind::RequestDeletionBackupRestoreReset.target_carries_owner_body(),
            "the deletion grammar carries an Owner body"
        );
        assert!(
            !ManagementIntentKind::ManageSchedule.target_carries_owner_body(),
            "other kinds keep their readable targets"
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
        assert!(
            !rendered.contains("quoted managed content"),
            "body redacted: {rendered}"
        );
        assert!(
            rendered.contains("Setup status"),
            "labels stay visible: {rendered}"
        );
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
            assert!(
                parse_credential_target(&target).is_none(),
                "credential parse rejects {raw:?}"
            );
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
            assert!(
                parse_consent_target(&target).is_none(),
                "consent parse rejects {raw:?}"
            );
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
                None,
                "the task grammar rejects {raw:?}"
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
            Some("C:\\Users\\ene\\workspace"),
            "the Owner-authored path stays verbatim, colons included"
        );
        for raw in ["workspace:", "task:abc", "setup:show", ""] {
            assert_eq!(
                parse_workspace_target(&ManagementTargetWire(String::from(raw))),
                None,
                "the workspace grammar rejects {raw:?}"
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
        let system = usage_cap_target("system", None, "daily_utc", "USD", 1_000_000);
        assert_eq!(system.0.as_str(), "cap:system:daily_utc:USD:1000000");
        assert_eq!(
            parse_usage_cap_target(&system),
            Some(UsageCapTarget {
                scope: String::from("system"),
                provider: None,
                window: String::from("daily_utc"),
                currency: String::from("USD"),
                limit_micros: 1_000_000,
            })
        );
        let provider = usage_cap_target("provider", Some("openai"), "monthly_utc", "USD", 42);
        assert_eq!(
            provider.0.as_str(),
            "cap:provider:openai:monthly_utc:USD:42"
        );
        assert_eq!(
            parse_usage_cap_target(&provider),
            Some(UsageCapTarget {
                scope: String::from("provider"),
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
                None,
                "the cap grammar rejects {raw:?} without guessing"
            );
        }
        // Zero is representable text; the owner refuses it as a limit, so the
        // grammar does not pre-decide that domain outcome.
        assert!(
            parse_usage_cap_target(&usage_cap_target("system", None, "daily_utc", "USD", 0))
                .is_some()
        );
    }
}
