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
    ManageSchedule,
    /// Deny or refuse rule/consent handling.
    DenyOrRefuse,
    ManageRuleConsentCap,
    ManageDevice,
    /// Credential configuration intent (values travel the protected
    /// Host-local path only, never this payload).
    ConfigureCredentialIntent,
    RequestDeletionBackupRestoreReset,
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
        formatter
            .debug_struct("ManagementIntent")
            .field("intent_id", &self.intent_id)
            .field("kind", &self.kind)
            .field("target", &self.target)
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
        SETUP_COMPLETE_TARGET, SETUP_SHOW_TARGET, consent_target, credential_target,
        parse_consent_target, parse_credential_target,
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
}
