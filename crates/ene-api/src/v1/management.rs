//! Setup management inlet: intent in, filtered view out (IPC §18).
//!
//! The Client expresses intent and reads filtered views; every acceptance,
//! confirmation, and high-privilege final check happens Host-side. Secrets,
//! judgment copies, and full internal conditions never appear in views.
//! High-privilege final confirmation additionally never travels this wire:
//! it stays on the Host-local trusted first-party surface (IPC §18.1).

use serde::{Deserialize, Serialize};

use super::refs::{BaseViewMark, CommandWireId, ManagementTargetWire, ViewMarkWire};

/// Management intent kinds (IPC §18.2). Stage 1 exercises the Setup range;
/// the remaining kinds arrive with their owners, which alone may accept
/// them. The kind name never decides the trust class: the Host classifies
/// by operation, target, and impact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ManagementIntentKind {
    /// Companion shutdown.
    StopCompanion,
    /// Companion deletion.
    DeleteCompanion,
    /// Task cancellation.
    CancelTask,
    /// Schedule management.
    ManageSchedule,
    /// Deny or refuse rule/consent handling.
    DenyOrRefuse,
    /// Rule, consent, and cap management.
    ManageRuleConsentCap,
    /// Device management.
    ManageDevice,
    /// Credential configuration intent (values travel the protected
    /// Host-local path only, never this payload).
    ConfigureCredentialIntent,
    /// Deletion, backup, restore, and reset requests.
    RequestDeletionBackupRestoreReset,
}

/// One management intent: idempotent by key, advisory by nature. The Host
/// maps it to a domain premise and answers with [`ManagementOutcome`];
/// the Client never self-declares the result.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ManagementIntent {
    /// Idempotency key: the envelope command ID family.
    pub intent_id: CommandWireId,
    /// What is being proposed.
    pub kind: ManagementIntentKind,
    /// Target reference: wire refs only, never control state.
    pub target: ManagementTargetWire,
    /// Display-revision mark the intent was built on. Required: staleness
    /// is checked, never defaulted to unconstrained.
    pub base_view: BaseViewMark,
    /// Owner intent record. Redacted from [`core::fmt::Debug`].
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
    /// Whether the intent originates from conversation or surface operation.
    pub origin: RationaleOrigin,
    /// Quoted correspondence. Redacted from [`core::fmt::Debug`].
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

/// Intent provenance vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RationaleOrigin {
    /// The intent came from conversation.
    Conversation,
    /// The intent came from management-surface operation.
    ManagementSurface,
}

/// Management outcome: an Ok-side domain outcome, never an error (IPC
/// §18.2, §24).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ManagementOutcome {
    /// Applied as a one-time approval. Never a standing rule.
    AppliedAsOneTime,
    /// Stored as a rule or similar, with its revision view.
    StoredAsRuleView {
        /// Revision view of what was stored.
        revision: ViewMarkWire,
    },
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

/// Filtered view request: which sections the Client wants to display.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ManagementViewRequest {
    /// Requested section kinds.
    pub sections: Vec<String>,
}

/// One filtered view section: short labels stay visible, bodies redact.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ViewSection {
    /// Section kind discriminator, display routing only.
    pub kind: String,
    /// Short section title. Labels stay visible in Debug.
    pub title: String,
    /// Section body. Display facts that may quote managed content:
    /// redacted from [`core::fmt::Debug`].
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

/// Filtered management view: revision mark plus sections. Secrets,
/// judgment copies, and full internal conditions are never included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagementView {
    /// Opaque display-revision mark.
    pub mark: ViewMarkWire,
    /// Filtered sections.
    pub sections: Vec<ViewSection>,
}

#[cfg(test)]
mod tests {
    use super::super::refs::{BaseViewMark, CommandWireId, ManagementTargetWire, ViewMarkWire};
    use super::{IntentRationaleWire, ManagementIntent, ManagementIntentKind, RationaleOrigin};
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
}
