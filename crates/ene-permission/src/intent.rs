//! Consent management-intent orchestration.
//!
//! The Host converts a management wire intent into [`AssignConsentIntent`]
//! premises; this module owns the consent-side decision order: parse the
//! base-view mark, hold when the route's credential is not fully present,
//! refuse a moved base, claim the same-route shortcut, and finally commit
//! through the compare-and-save transaction. The intent journal stays
//! write-once in every path, so a reused id observes one outcome forever.

use crate::{
    ConsentCommitOutcome, ConsentRecord, ConsentRepository, ConsentRevision, IntentFingerprint,
    IntentOutcome, IntentOutcomeRecord, IntentOutcomeRepository, IntentResolution,
    PermissionTechnicalError, ShortcutIntentOutcome, consent_mark_rev,
};

/// Parsed consent base-view mark: a compare-and-save expectation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseViewExpectation {
    /// The mark (`"consent-none"`) expects no stored row.
    ExpectEmpty,
    /// The mark (`"consent-rev-N"`) expects the stored row at revision `N`.
    ExpectRevision(String, ConsentRevision),
    /// The mark is stale on its face: unparseable, or a revision claim with
    /// no stored row.
    FaceStale,
}

/// Parses a consent base-view mark into a compare-and-save expectation.
///
/// `"consent-none"` expects no stored row; `"consent-rev-N"` expects the
/// loaded current record at revision `N`.
#[must_use]
pub fn base_view_expectation(
    base_view: &str,
    current: Option<&ConsentRecord>,
) -> BaseViewExpectation {
    if base_view == "consent-none" {
        return BaseViewExpectation::ExpectEmpty;
    }
    let Some(revision_text) = base_view.strip_prefix("consent-rev-") else {
        return BaseViewExpectation::FaceStale;
    };
    let Ok(revision_number) = revision_text.parse::<u64>() else {
        return BaseViewExpectation::FaceStale;
    };
    let Some(stored) = current else {
        return BaseViewExpectation::FaceStale;
    };
    BaseViewExpectation::ExpectRevision(
        stored.id.clone(),
        ConsentRevision::from_u64(revision_number),
    )
}

/// Owner-side premises of one consent-assign intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignConsentIntent {
    /// Provider the route binds.
    pub provider: String,
    /// Model the route binds.
    pub model: String,
    /// Credential id the route requires.
    pub credential_id: String,
    /// Base-view mark the intent was built on.
    pub base_view: String,
    /// Whether the credential id resolves to a registered, bearer-backed
    /// ref (computed by the Host, which alone can cross owners).
    pub credential_present: bool,
    /// Durable intent fingerprint.
    pub fingerprint: IntentFingerprint,
}

/// Result of one consent-assign attempt.
///
/// `Decided` is the fresh outcome this call recorded; `Replay`/`Conflict`
/// carry the immutable journal row so the caller answers from durable
/// state, never from a locally decided outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignConsentResolution {
    /// This call decided and recorded the outcome.
    Decided(IntentOutcome),
    /// Exact fingerprint replay; nothing changed.
    Replay(IntentOutcomeRecord),
    /// Same id, different fingerprint; nothing changed.
    Conflict(IntentOutcomeRecord),
}

/// Assigns the consent route from owner-side premises.
///
/// Order: current read, mark parse (face-stale records its stale answer),
/// credential presence (absent records a clarification), base freshness
/// (moved records the stale answer), atomic same-route shortcut, then the
/// compare-and-save commit that bumps the revision. Every deciding path
/// records its journal row first, so a retry replays it.
pub async fn assign_consent(
    intents: &impl IntentOutcomeRepository,
    consents: &impl ConsentRepository,
    intent: AssignConsentIntent,
) -> Result<AssignConsentResolution, PermissionTechnicalError> {
    let current = consents.load_current().await?;
    let expected = match base_view_expectation(&intent.base_view, current.as_ref()) {
        BaseViewExpectation::ExpectEmpty => None,
        BaseViewExpectation::ExpectRevision(id, revision) => Some((id, revision)),
        BaseViewExpectation::FaceStale => {
            return record_decided(
                intents,
                intent.fingerprint,
                IntentOutcome::StaleBaseView {
                    current: current_mark(current.as_ref()),
                },
            )
            .await;
        }
    };
    if !intent.credential_present {
        return record_decided(
            intents,
            intent.fingerprint,
            IntentOutcome::NeedsClarification,
        )
        .await;
    }
    // The base premise is enforced before the shortcut: a stale base with a
    // coincidentally equal route must answer stale, never silent success.
    let base_fresh = match (&expected, current.as_ref()) {
        (None, None) => true,
        (Some((id, revision)), Some(record)) => record.id == *id && record.rev == *revision,
        (None, Some(_)) | (Some(_), None) => false,
    };
    if !base_fresh {
        return record_decided(
            intents,
            intent.fingerprint,
            IntentOutcome::StaleBaseView {
                current: current_mark(current.as_ref()),
            },
        )
        .await;
    }
    match intents
        .shortcut_with_intent(
            intent.provider.clone(),
            intent.model.clone(),
            intent.credential_id.clone(),
            intent.fingerprint.clone(),
        )
        .await?
    {
        IntentResolution::Decided(ShortcutIntentOutcome::Hit { current }) => {
            return Ok(AssignConsentResolution::Decided(
                IntentOutcome::StoredAsRuleView {
                    revision: current.rev.as_u64().to_string(),
                },
            ));
        }
        IntentResolution::Decided(ShortcutIntentOutcome::Miss { .. }) => {}
        IntentResolution::Replay(stored) => {
            return Ok(AssignConsentResolution::Replay(stored));
        }
        IntentResolution::Conflict(stored) => {
            return Ok(AssignConsentResolution::Conflict(stored));
        }
    }
    let next_rev = match current.as_ref() {
        Some(record) => record.rev.as_u64().saturating_add(1),
        None => 1,
    };
    let id = match current {
        Some(record) => record.id,
        None => ene_primitive::RawId::new().as_uuid().to_string(),
    };
    let record = ConsentRecord {
        id,
        rev: ConsentRevision::from_u64(next_rev),
        provider: intent.provider,
        model: intent.model,
        credential_id: intent.credential_id,
    };
    match intents
        .assign_with_intent(expected, record, intent.fingerprint)
        .await?
    {
        IntentResolution::Decided(ConsentCommitOutcome::Committed { record }) => Ok(
            AssignConsentResolution::Decided(IntentOutcome::StoredAsRuleView {
                revision: record.rev.as_u64().to_string(),
            }),
        ),
        IntentResolution::Decided(ConsentCommitOutcome::StaleCurrent { current }) => Ok(
            AssignConsentResolution::Decided(IntentOutcome::StaleBaseView {
                current: current_mark(current.as_ref()),
            }),
        ),
        IntentResolution::Replay(stored) => Ok(AssignConsentResolution::Replay(stored)),
        IntentResolution::Conflict(stored) => Ok(AssignConsentResolution::Conflict(stored)),
    }
}

/// Renders the current consent mark for a possibly absent record.
fn current_mark(current: Option<&ConsentRecord>) -> String {
    consent_mark_rev(current.map(|record| record.rev.as_u64()))
}

/// Records one locally decided outcome and maps the write-once resolution.
async fn record_decided(
    intents: &impl IntentOutcomeRepository,
    fingerprint: IntentFingerprint,
    outcome: IntentOutcome,
) -> Result<AssignConsentResolution, PermissionTechnicalError> {
    let answer = outcome.clone();
    let resolution = intents
        .record_intent_outcome(IntentOutcomeRecord {
            fingerprint,
            outcome,
        })
        .await?;
    Ok(match resolution {
        IntentResolution::Decided(()) => AssignConsentResolution::Decided(answer),
        IntentResolution::Replay(stored) => AssignConsentResolution::Replay(stored),
        IntentResolution::Conflict(stored) => AssignConsentResolution::Conflict(stored),
    })
}
