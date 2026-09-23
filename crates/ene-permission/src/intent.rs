use crate::{
    CapabilityKind, ConsentCommitOutcome, ConsentRecord, ConsentRepository, ConsentRevision,
    IntentFingerprint, IntentOutcome, IntentOutcomeRecord, IntentOutcomeRepository,
    IntentResolution, PermissionTechnicalError, ShortcutIntentOutcome, consent_mark,
    parse_consent_mark,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseViewExpectation {
    ExpectEmpty,
    ExpectRevision(String, ConsentRevision),
    FaceStale,
}

#[must_use]
pub fn base_view_expectation(
    capability: CapabilityKind,
    base_view: &str,
    current: Option<&ConsentRecord>,
) -> BaseViewExpectation {
    match parse_consent_mark(base_view, capability) {
        Some(None) => BaseViewExpectation::ExpectEmpty,
        Some(Some(revision_number)) => {
            let Some(stored) = current else {
                return BaseViewExpectation::FaceStale;
            };
            BaseViewExpectation::ExpectRevision(
                stored.id.clone(),
                ConsentRevision::from_u64(revision_number),
            )
        }
        None => BaseViewExpectation::FaceStale,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignConsentIntent {
    pub capability: CapabilityKind,
    pub provider: String,
    pub model: String,
    pub credential_id: String,
    pub base_view: String,
    pub credential_present: bool,
    pub fingerprint: IntentFingerprint,
}

pub async fn assign_consent(
    intents: &impl IntentOutcomeRepository,
    consents: &impl ConsentRepository,
    intent: AssignConsentIntent,
) -> Result<IntentResolution<IntentOutcome>, PermissionTechnicalError> {
    let current = consents.load_current(intent.capability).await?;
    let expected =
        match base_view_expectation(intent.capability, &intent.base_view, current.as_ref()) {
            BaseViewExpectation::ExpectEmpty => None,
            BaseViewExpectation::ExpectRevision(id, revision) => Some((id, revision)),
            BaseViewExpectation::FaceStale => {
                return record_decided(
                    intents,
                    intent.fingerprint,
                    IntentOutcome::StaleBaseView {
                        current: current_mark(intent.capability, current.as_ref()),
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
                current: current_mark(intent.capability, current.as_ref()),
            },
        )
        .await;
    }
    match intents
        .shortcut_with_intent(
            intent.capability,
            intent.provider.clone(),
            intent.model.clone(),
            intent.credential_id.clone(),
            intent.fingerprint.clone(),
        )
        .await?
    {
        IntentResolution::Decided(ShortcutIntentOutcome::Hit { current }) => {
            return Ok(IntentResolution::Decided(IntentOutcome::StoredAsRuleView {
                revision: consent_mark(intent.capability, Some(current.rev.as_u64())),
            }));
        }
        IntentResolution::Decided(ShortcutIntentOutcome::Miss) => {}
        IntentResolution::Replay(stored) => {
            return Ok(IntentResolution::Replay(stored));
        }
        IntentResolution::Conflict(stored) => {
            return Ok(IntentResolution::Conflict(stored));
        }
    }
    let next_rev = match current.as_ref() {
        Some(record) => match record.rev.checked_next() {
            Some(next) => next,
            None => {
                return record_decided(
                    intents,
                    intent.fingerprint,
                    IntentOutcome::RevisionExhausted,
                )
                .await;
            }
        },
        None => ConsentRevision::from_u64(1),
    };
    let id = match current {
        Some(record) => record.id,
        None => ene_primitive::RawId::new().as_uuid().to_string(),
    };
    let record = ConsentRecord {
        capability: intent.capability,
        id,
        rev: next_rev,
        provider: intent.provider,
        model: intent.model,
        credential_id: intent.credential_id,
    };
    match intents
        .assign_with_intent(expected, record, intent.fingerprint)
        .await?
    {
        IntentResolution::Decided(ConsentCommitOutcome::Committed { record }) => {
            Ok(IntentResolution::Decided(IntentOutcome::StoredAsRuleView {
                revision: consent_mark(intent.capability, Some(record.rev.as_u64())),
            }))
        }
        IntentResolution::Decided(ConsentCommitOutcome::StaleCurrent { current }) => {
            Ok(IntentResolution::Decided(IntentOutcome::StaleBaseView {
                current: current_mark(intent.capability, current.as_ref()),
            }))
        }
        IntentResolution::Replay(stored) => Ok(IntentResolution::Replay(stored)),
        IntentResolution::Conflict(stored) => Ok(IntentResolution::Conflict(stored)),
    }
}

fn current_mark(capability: CapabilityKind, current: Option<&ConsentRecord>) -> String {
    consent_mark(capability, current.map(|record| record.rev.as_u64()))
}

async fn record_decided(
    intents: &impl IntentOutcomeRepository,
    fingerprint: IntentFingerprint,
    outcome: IntentOutcome,
) -> Result<IntentResolution<IntentOutcome>, PermissionTechnicalError> {
    let answer = outcome.clone();
    let resolution = intents
        .record_intent_outcome(IntentOutcomeRecord {
            fingerprint,
            outcome,
        })
        .await?;
    Ok(match resolution {
        IntentResolution::Decided(()) => IntentResolution::Decided(answer),
        IntentResolution::Replay(stored) => IntentResolution::Replay(stored),
        IntentResolution::Conflict(stored) => IntentResolution::Conflict(stored),
    })
}
