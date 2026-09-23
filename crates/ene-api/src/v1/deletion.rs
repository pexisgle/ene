//! Targeted Deletion management surface (IPC §18, targeted-deletion-lifecycle
//! §4 and §15).
//!
//! Three DTO groups live here:
//!
//! - the advisory request target grammar `deletion:{purpose}:{exact-text}`,
//!   shared by the Client builder and the authoritative Host parse;
//! - the bounded, body-free operation status view: phase, purpose, hold
//!   class, current sweep, and participant progress;
//! - nothing that can authorize an operation. The Client intent only ever
//!   stages a request; the destructive final confirmation is established on
//!   the Host-local trusted first-party surface (IPC §18.1) and never travels
//!   this wire.
//!
//! The exact text is Owner body. [`DeletionTargetRequest`] keeps it out of
//! `Debug`, the status view carries no target material at all, and the Host
//! journals the intent family and purpose without the body (the durable staged
//! request is the single scope record).

use serde::{Deserialize, Serialize};

use super::refs::{
    DeletionOperationWireRef, DeletionStatusCursorWire, ManagementTargetWire, ViewMarkWire,
};

/// Fixed prefix of one Targeted Deletion management target:
/// `deletion:{purpose}:{exact-text}`. The purpose token comes from a closed
/// set; the exact text is the remainder verbatim (it may contain `':'`), so
/// the grammar never needs an escaping layer the two sides could disagree on.
pub const DELETION_TARGET_PREFIX: &str = "deletion:";

/// Upper bound on one exact-text target. The target is a keyword or short
/// phrase; the bound keeps intent parsing, staging, and the eventual
/// mechanical search bounded upstream instead of truncating later.
pub const DELETION_EXACT_TEXT_MAX_BYTES: usize = 1024;

/// Fixed prefix of a Host-issued deletion-status page cursor. The Client
/// echoes the whole value; a cursor that does not carry this prefix is not a
/// deletion-status cursor and is rejected, never silently reused.
pub const DELETION_STATUS_CURSOR_PREFIX: &str = "deletion-status:";

/// Declared purpose of one Targeted Deletion request. The closed set is the
/// admission vocabulary (lifecycle §4); the final confirmation premise is the
/// Host-local Owner decision, not this token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeletionPurposeWire {
    Privacy,
    Security,
}

impl DeletionPurposeWire {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Privacy => "privacy",
            Self::Security => "security",
        }
    }

    /// Unknown purpose tokens are outside the closed set, never defaulted.
    #[must_use]
    pub fn from_name(token: &str) -> Option<Self> {
        match token {
            "privacy" => Some(Self::Privacy),
            "security" => Some(Self::Security),
            _ => None,
        }
    }
}

/// Plain constructor: it does not validate. The Host parse stays
/// authoritative, and the exact text is the Owner body — never render it
/// through a log or a view.
#[must_use]
pub fn deletion_target(purpose: DeletionPurposeWire, exact_text: &str) -> ManagementTargetWire {
    ManagementTargetWire(format!(
        "{DELETION_TARGET_PREFIX}{}:{exact_text}",
        purpose.as_str()
    ))
}

/// Exact rule: strip the `deletion:` prefix, split once on `':'`, parse the
/// purpose token from the closed set, and require a non-empty remainder no
/// longer than [`DELETION_EXACT_TEXT_MAX_BYTES`], else [`None`]. The remainder
/// is the exact Owner-authored text verbatim.
#[must_use]
pub fn parse_deletion_target(target: &ManagementTargetWire) -> Option<DeletionTargetRequest> {
    let rest = target.0.strip_prefix(DELETION_TARGET_PREFIX)?;
    let (purpose, exact_text) = rest.split_once(':')?;
    let purpose = DeletionPurposeWire::from_name(purpose)?;
    if exact_text.is_empty() || exact_text.len() > DELETION_EXACT_TEXT_MAX_BYTES {
        return None;
    }
    Some(DeletionTargetRequest {
        purpose,
        exact_text: exact_text.to_owned(),
    })
}

/// One parsed Targeted Deletion request. Advisory: the Host re-validates the
/// target, purpose, and current deletion surface mark before anything durable
/// happens, and only the Host-local trusted confirmation makes it destructive.
///
/// The exact text is the Owner body, so it has no public field, no `Debug`
/// rendering, and no `Serialize`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct DeletionTargetRequest {
    purpose: DeletionPurposeWire,
    exact_text: String,
}

impl DeletionTargetRequest {
    #[must_use]
    pub fn purpose(&self) -> DeletionPurposeWire {
        self.purpose
    }

    /// Protected Owner-body access for the Host's admission mapping only.
    /// Never a display, `Debug`, log, or view representation.
    #[must_use]
    pub fn exact_text(&self) -> &str {
        &self.exact_text
    }
}

impl core::fmt::Debug for DeletionTargetRequest {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DeletionTargetRequest")
            .field("purpose", &self.purpose)
            .field("exact_text", &"[redacted]")
            .finish()
    }
}

/// First-party bounded read of the Targeted Deletion operation surface.
/// `limit` is 1..=50 (default 50); `cursor` is a Host-issued
/// [`DeletionStatusCursorWire`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionStatusRequest {
    #[serde(default)]
    pub cursor: Option<DeletionStatusCursorWire>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Answer of [`DeletionStatusRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeletionStatusResponse {
    /// One bounded page plus the current deletion surface mark.
    Page(DeletionStatusPage),
    /// The durable surface is unreadable (torn canonical state or a storage
    /// failure). Explicitly not an empty page: an unavailable status must
    /// never read as "nothing is being deleted".
    Unavailable,
}

/// One page: current values of a single read transaction, not a multi-page
/// snapshot. `next_cursor` is present while a later page may exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionStatusPage {
    /// Display-revision mark the page was built on. An intent built on this
    /// mark is checked against the live surface; a moved surface answers
    /// `StaleBaseView` and requires a re-read.
    pub mark: ViewMarkWire,
    pub operations: Vec<DeletionOperationStatusView>,
    pub next_cursor: Option<DeletionStatusCursorWire>,
}

/// One operation's bounded status. No target body, no search material, no
/// credential, and no participant payload beyond the participant-owned
/// progress tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionOperationStatusView {
    /// Canonical operation identity rendering. Display only; echo it, never
    /// re-identify an operation for mutation with it.
    pub operation: DeletionOperationWireRef,
    pub phase: DeletionPhaseWire,
    pub purpose: DeletionPurposeWire,
    /// Display copy of the admission timestamp (RFC 3339).
    pub started_at: String,
    /// Current sweep generation of this operation.
    pub sweep: u64,
    /// Hold class while the operation is retryably unfinished.
    pub hold: Option<DeletionHoldWire>,
    pub participants: DeletionParticipantReportWire,
}

/// Operation lifecycle phase (lifecycle §5). `Completed` is the only terminal
/// phase and means the whole surface — not one participant — finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeletionPhaseWire {
    Active,
    Held,
    Finalizing,
    Completed,
}

impl DeletionPhaseWire {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Held => "held",
            Self::Finalizing => "finalizing",
            Self::Completed => "completed",
        }
    }
}

/// Retryable hold class of an unfinished operation. Never a terminal failure:
/// a privacy operation stays unfinished until the Owner decides or recovery
/// succeeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeletionHoldWire {
    Unavailable,
    GenerationExhausted,
}

impl DeletionHoldWire {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::GenerationExhausted => "generation-exhausted",
        }
    }
}

/// Participant progress of one operation.
///
/// The closed progress vocabulary belongs to the participant owner (the
/// `deletion_participant` registry); this DTO carries its display tokens.
/// [`NotReported`](Self::NotReported) is the honest value while the
/// participant owner read is not connected: the view never fabricates
/// `Pending`/`Verified`, and an empty report is never "all complete".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeletionParticipantReportWire {
    NotReported,
    Reported(Vec<DeletionParticipantStatusWire>),
}

/// One participant's reported progress. The tokens are display copies of the
/// participant owner's durable classes; this DTO closes no set of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionParticipantStatusWire {
    /// Participant owner token, as the participant owner names itself.
    pub owner: String,
    /// Progress class token (for example `pending`, `running`,
    /// `local-complete`, `verified`, `held:<class>`).
    pub progress: String,
    /// Sweep generation the progress belongs to. A stale sweep never
    /// completes the current one.
    pub sweep: u64,
}

/// Host-minted correlation identity of one Client local-erasure demand
/// (IPC §17.1). The Host mints it per demand, the Client echoes it on the
/// [`LocalErasureResult`], and a result whose identity names no outstanding
/// demand for that incarnation is ignored. It is an opaque string, never
/// parsed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeletionDemandWireId(pub String);

/// One class of Client-local transient data a demand can wipe (IPC §17.2).
///
/// The closed set names the current first-party Client surface only; a class
/// is wiped as a whole because the current Client keeps no ranged local copy
/// index. Finer targeting (a time range or a per-item local-copy identity)
/// is added by the slice that first keeps such an index — the Host never
/// guesses a range for a Client that does not track one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClientTempClass {
    /// Received-but-unconsumed presentation material: undelivered summaries,
    /// excerpts, and history pages held in the Client's deferred/display
    /// buffer before the user has seen them.
    PresentationBuffer,
    /// The local input draft the user has typed but not submitted. It may
    /// already quote the target and must not survive as a local copy.
    InputDraft,
}

impl ClientTempClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PresentationBuffer => "presentation-buffer",
            Self::InputDraft => "input-draft",
        }
    }

    /// Parses the [`Self::as_str`] vocabulary, closed world. An unknown class
    /// is refused, never silently mapped to "no local copy".
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "presentation-buffer" => Some(Self::PresentationBuffer),
            "input-draft" => Some(Self::InputDraft),
            _ => None,
        }
    }
}

/// One target of a [`DeletionDemand`].
///
/// Targets name local data by class, never by the mechanical target text or a
/// re-derivable copy of it: the wire never carries the Owner's body or the
/// Host's search material (IPC §17.2, §23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeletionTargetWire {
    WipeClass { class: ClientTempClass },
}

/// Host → Client local-erasure demand (IPC §17.1, lifecycle §9).
///
/// It is a bounded command for one Client incarnation: wipe the named local
/// transient classes and report what was wiped and what could not be
/// verified. It carries no target body, no search material, and no authority
/// to declare the system-wide deletion complete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionDemand {
    /// Host-minted demand correlation; the result echoes it verbatim.
    pub demand: DeletionDemandWireId,
    /// Operation identity rendering (the same projection the status view
    /// uses). Correlation only: the Client cannot mutate by echoing it.
    pub operation: DeletionOperationWireRef,
    /// Sweep the demand belongs to. A result for an older sweep never
    /// completes the current one.
    pub sweep: u64,
    pub targets: Vec<DeletionTargetWire>,
}

/// Client → Host report of one bounded local-erasure demand (IPC §17.1).
///
/// It reports what this Client wiped and what it could not verify. An empty
/// `unverified` list is the participant's own local completion premise, never
/// the system-wide completion (§10: local completion and global completion
/// are different facts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalErasureResult {
    /// Echo of the [`DeletionDemand::demand`] this result answers.
    pub demand: DeletionDemandWireId,
    /// Operation the Client believed it was demanded for. The Host refuses a
    /// mismatch instead of adopting a foreign operation's erasure.
    pub operation: DeletionOperationWireRef,
    /// Sweep the Client believed it was demanded for.
    pub sweep: u64,
    pub wiped: Vec<ClientTempClass>,
    /// Classes the Client could not verify as wiped (for example a local copy
    /// outside its management boundary). A non-empty list keeps the
    /// participant unfinished: it is never read as success.
    pub unverified: Vec<ClientTempClass>,
}

#[cfg(test)]
mod tests {
    use super::super::refs::ManagementTargetWire;
    use super::{
        ClientTempClass, DELETION_EXACT_TEXT_MAX_BYTES, DELETION_TARGET_PREFIX, DeletionPhaseWire,
        DeletionPurposeWire, deletion_target, parse_deletion_target,
    };

    #[test]
    fn deletion_target_roundtrips_with_colons_and_unicode() {
        for text in [
            "secret",
            "with:colons:inside",
            "日本語のキーワード",
            " leading and trailing ",
        ] {
            let target = deletion_target(DeletionPurposeWire::Privacy, text);
            let parsed = parse_deletion_target(&target).expect("the built target must parse");
            assert_eq!(parsed.purpose(), DeletionPurposeWire::Privacy);
            assert_eq!(parsed.exact_text(), text);
        }
        let security = deletion_target(DeletionPurposeWire::Security, "leaked-key");
        assert_eq!(security.0.as_str(), "deletion:security:leaked-key");
        assert_eq!(
            parse_deletion_target(&security).map(|request| request.purpose()),
            Some(DeletionPurposeWire::Security)
        );
    }

    #[test]
    fn deletion_target_parser_rejects_other_families_and_shapes() {
        for raw in [
            "deletion:",
            "deletion:privacy",
            "deletion:privacy:",
            "deletion:unknown:secret",
            "deletion::secret",
            "credential:openai:personal",
            "setup:show",
            "task:not-a-uuid",
            "",
        ] {
            let target = ManagementTargetWire(String::from(raw));
            assert!(
                parse_deletion_target(&target).is_none(),
                "the deletion grammar rejects {raw:?}"
            );
        }
        assert_eq!(DELETION_TARGET_PREFIX, "deletion:");
    }

    #[test]
    fn deletion_target_bounds_the_exact_text() {
        let at_limit = "x".repeat(DELETION_EXACT_TEXT_MAX_BYTES);
        assert!(
            parse_deletion_target(&deletion_target(DeletionPurposeWire::Privacy, &at_limit))
                .is_some(),
            "a target at the bound is admissible"
        );
        let over = "x".repeat(DELETION_EXACT_TEXT_MAX_BYTES + 1);
        assert!(
            parse_deletion_target(&deletion_target(DeletionPurposeWire::Privacy, &over)).is_none(),
            "a target over the bound is refused, never truncated"
        );
    }

    #[test]
    fn parsed_request_debug_redacts_the_owner_body() {
        let request =
            parse_deletion_target(&deletion_target(DeletionPurposeWire::Privacy, "raw secret"))
                .expect("the built target must parse");
        let rendered = format!("{request:?}");
        assert!(
            !rendered.contains("raw secret"),
            "the exact text never renders through Debug: {rendered}"
        );
        assert!(
            rendered.contains("Privacy"),
            "the purpose stays visible: {rendered}"
        );
        assert!(
            !rendered.to_lowercase().contains("exact_text: \"raw"),
            "the body is redacted: {rendered}"
        );
    }

    #[test]
    fn phase_tokens_are_stable() {
        assert_eq!(DeletionPhaseWire::Active.as_str(), "active");
        assert_eq!(DeletionPhaseWire::Held.as_str(), "held");
        assert_eq!(DeletionPhaseWire::Finalizing.as_str(), "finalizing");
        assert_eq!(DeletionPhaseWire::Completed.as_str(), "completed");
    }

    #[test]
    fn client_temp_classes_round_trip_through_their_closed_vocabulary() {
        for class in [
            ClientTempClass::PresentationBuffer,
            ClientTempClass::InputDraft,
        ] {
            assert_eq!(ClientTempClass::from_name(class.as_str()), Some(class));
        }
        assert_eq!(ClientTempClass::from_name("screen-scrollback"), None);
    }
}
