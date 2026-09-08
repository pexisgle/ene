//! Setup management inlet: intent in, filtered view out.
//!
//! The Client expresses intent and reads filtered views; every acceptance,
//! confirmation, and high-privilege final check happens Host-side. Secrets,
//! judgment copies, and full internal conditions never appear in views.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Setup intent kinds for Stage 1. Stored-rule views and broader control
/// intents arrive with their owners in later stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SetupIntentKind {
    /// General settings configuration.
    ConfigureGeneralSettings,
    /// Inference provider selection.
    SelectProvider,
    /// Credential registration intent (values travel the protected
    /// Host-local path only, never this payload).
    RegisterCredentialIntent,
    /// Setup completion declaration for Host enforcement.
    CompleteSetup,
}

/// One management intent: idempotent by key, advisory by nature. The Host
/// may apply, clarify, deny, or hold; the Client never self-declares the
/// result (least of all Setup completeness).
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ManagementIntent {
    /// Idempotency key, minted per intent.
    pub intent_id: Uuid,
    /// What is being proposed.
    pub kind: SetupIntentKind,
    /// Opaque target reference. Echoed, never interpreted.
    pub target: String,
    /// Base view the intent was built on, for staleness checks.
    pub base_view: Option<String>,
    /// Owner-written rationale. Free text: redacted from
    /// [`core::fmt::Debug`].
    pub rationale: Option<String>,
}

impl core::fmt::Debug for ManagementIntent {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ManagementIntent")
            .field("intent_id", &self.intent_id)
            .field("kind", &self.kind)
            .field("target", &self.target)
            .field("base_view", &self.base_view)
            .field("rationale", &self.rationale.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

/// Management outcome: an Ok-side domain outcome, never an error.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ManagementOutcome {
    /// Applied as a one-time change. Never a standing rule.
    AppliedAsOneTime,
    /// The Host needs clarification before deciding.
    NeedsClarification {
        /// Operational detail. Never a secret or a body copy.
        detail: String,
    },
    /// Denied at the permission/control boundary.
    DeniedByBoundary {
        /// Operational reason. Never a secret or a body copy.
        reason: String,
    },
    /// The base view moved underneath the intent.
    StaleBaseView {
        /// Current mark the sender should build on next time.
        current: String,
    },
    /// Held by an ongoing operation.
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
    pub mark: super::refs::ViewMarkWire,
    /// Filtered sections.
    pub sections: Vec<ViewSection>,
}

#[cfg(test)]
mod tests {
    use super::{ManagementIntent, SetupIntentKind, ViewSection};
    use uuid::Uuid;

    #[test]
    fn intent_debug_redacts_rationale() {
        let intent = ManagementIntent {
            intent_id: Uuid::new_v4(),
            kind: SetupIntentKind::CompleteSetup,
            target: String::from("setup"),
            base_view: None,
            rationale: Some(String::from("because I pasted something private")),
        };
        let rendered = format!("{intent:?}");
        assert!(
            !rendered.contains("pasted something private"),
            "rationale redacted: {rendered}"
        );
        assert!(rendered.contains("setup"), "refs stay visible: {rendered}");
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
