//! Owner-local Targeted Deletion erasure for the control, credential, and
//! presence owners (Stage 6 A3d): bounded erase + verification, no collateral
//! mutation, no external credential effect, and stale-generation refusal.
//!
//! The three ports share one method name, so every call is trait-qualified:
//! `Store` implements all of them and a bare call would be ambiguous.

use super::*;
use ene_credential::{CredentialErasureOutcome, CredentialErasureRepository};
use ene_permission::{
    IntentOutcome, IntentOutcomeRecord, PermissionErasureOutcome, PermissionErasureRepository,
};
use ene_presence::{PresenceAttribution, PresenceErasureOutcome, PresenceErasureRepository};

/// Commits one consent row with the caller-supplied route parts.
async fn save_route_consent(
    store: &Store,
    capability: CapabilityKind,
    provider: &str,
    model: &str,
    credential_id: &str,
    intent_id: &str,
) {
    let record = ConsentRecord {
        capability,
        id: String::from("consent-fixture"),
        rev: ConsentRevision::from_u64(1),
        provider: provider.to_owned(),
        model: model.to_owned(),
        credential_id: credential_id.to_owned(),
    };
    let fingerprint = IntentFingerprint {
        intent_id: intent_id.to_owned(),
        kind: String::from("assign"),
        target: format!("consent:{}", capability.as_str()),
        base: String::from("consent-none"),
        rationale_origin: String::from("management-surface"),
        rationale_quote: None,
    };
    let outcome = store
        .assign_with_intent(None, record, fingerprint)
        .await
        .expect("the consent fixture must commit");
    assert!(matches!(outcome, IntentResolution::Decided(_)));
}

async fn record_decision(
    store: &Store,
    intent_id: &str,
    target: &str,
    rationale_quote: Option<&str>,
) {
    let resolution = store
        .record_intent_outcome(IntentOutcomeRecord {
            fingerprint: IntentFingerprint {
                intent_id: intent_id.to_owned(),
                kind: String::from("assign"),
                target: target.to_owned(),
                base: String::from("consent-none"),
                rationale_origin: String::from("conversation"),
                rationale_quote: rationale_quote.map(str::to_owned),
            },
            outcome: IntentOutcome::NeedsClarification,
        })
        .await
        .expect("the journal fixture must commit");
    assert!(matches!(resolution, IntentResolution::Decided(())));
}

fn contains_target(value: &str, target: &str) -> bool {
    value.contains(target)
}

#[tokio::test]
async fn permission_erasure_redacts_owner_text_and_invalidates_target_bearing_consent() {
    let store = open_memory().await.unwrap();
    let target = "midnight-plan";
    // Decision journal: one row carries the target in the quoted rationale,
    // one in the intent target (a workspace path), one is unrelated.
    record_decision(&store, "intent-quoted", "workspace:prod", Some(target)).await;
    record_decision(
        &store,
        "intent-target",
        &format!("workspace:/home/owner/{target}"),
        None,
    )
    .await;
    record_decision(
        &store,
        "intent-unrelated",
        "workspace:prod",
        Some("unrelated"),
    )
    .await;
    // Consent: the dialogue route carries the target through its credential
    // id; the learning route is unrelated and must survive byte-identical.
    save_route_consent(
        &store,
        CapabilityKind::Dialogue,
        "acme",
        "dialogue-1",
        target,
        "assign-dialogue",
    )
    .await;
    save_route_consent(
        &store,
        CapabilityKind::Learning,
        "acme",
        "learn-1",
        "learn-cred",
        "assign-learning",
    )
    .await;
    let unrelated_learning = store.load_current(CapabilityKind::Learning).await.unwrap();

    let current = super::preservation::admit(&store, target, vec![]).await;
    let condition = current.condition();
    let applied = PermissionErasureRepository::erase_target_text(&store, condition, target)
        .await
        .expect("the permission erasure must answer");
    assert_eq!(
        applied,
        PermissionErasureOutcome::Applied {
            erased: 3,
            remainder: 0
        },
        "two journal rows and one consent row carry the target"
    );

    // Remainder verification: no permission-owned value carries the target.
    // The journal rows keep their identity and decided outcome, so a retried
    // intent id still cannot re-execute; the body text is gone.
    let quoted = store
        .lookup_intent_outcome("intent-quoted")
        .await
        .unwrap()
        .expect("the decided row survives");
    assert_eq!(
        quoted.outcome,
        IntentOutcome::NeedsClarification,
        "the decided outcome is preserved, not reopened"
    );
    assert!(
        quoted
            .fingerprint
            .rationale_quote
            .as_deref()
            .is_some_and(|quote| !contains_target(quote, target)),
        "the quoted rationale is redacted"
    );
    let targeted = store
        .lookup_intent_outcome("intent-target")
        .await
        .unwrap()
        .expect("the decided row survives");
    assert!(!contains_target(&targeted.fingerprint.target, target));
    let unrelated = store
        .lookup_intent_outcome("intent-unrelated")
        .await
        .unwrap()
        .expect("the unrelated decided row survives");
    assert_eq!(
        unrelated.fingerprint.rationale_quote.as_deref(),
        Some("unrelated")
    );

    // The current control outcome is fail-closed: the target-bearing consent
    // is invalidated, never rewritten into a different route, and erasure is
    // never read as "allowed". Unrelated consent is untouched.
    assert_eq!(
        store.load_current(CapabilityKind::Dialogue).await.unwrap(),
        None
    );
    assert_eq!(
        store.load_current(CapabilityKind::Learning).await.unwrap(),
        unrelated_learning
    );

    // A duplicate demand has no second semantic effect.
    let again = PermissionErasureRepository::erase_target_text(&store, condition, target)
        .await
        .expect("the repeated pass must answer");
    assert_eq!(
        again,
        PermissionErasureOutcome::Applied {
            erased: 0,
            remainder: 0
        }
    );

    // A retried intent id whose body carried the target observes a conflict,
    // never a fresh decision: erasing the text must not reopen a decided
    // authorization.
    let retried = store
        .record_intent_outcome(IntentOutcomeRecord {
            fingerprint: IntentFingerprint {
                intent_id: String::from("intent-quoted"),
                kind: String::from("assign"),
                target: String::from("workspace:prod"),
                base: String::from("consent-none"),
                rationale_origin: String::from("conversation"),
                rationale_quote: Some(target.to_owned()),
            },
            outcome: IntentOutcome::AppliedAsOneTime,
        })
        .await
        .expect("the retried intent must answer");
    assert!(
        matches!(retried, IntentResolution::Conflict(_)),
        "a body-carrying retry after redaction is a conflict, got {retried:?}"
    );
    let stored = store
        .lookup_intent_outcome("intent-quoted")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.outcome,
        IntentOutcome::NeedsClarification,
        "the first decision is never rewritten by a retry"
    );
    assert!(!contains_target(
        stored.fingerprint.rationale_quote.as_deref().unwrap_or(""),
        target
    ));
}

#[tokio::test]
async fn credential_erasure_removes_metadata_without_touching_protected_values() {
    use ene_credential::{CredentialStore as _, available_credential};

    let store = open_memory().await.unwrap();
    let memory = MemoryCredentialStore::new();
    let target = "midnight-plan";
    // Two usable refs and one target-bearing pending registration.
    approve_pair(
        &store,
        "acme",
        target,
        "target-bearer-value",
        "register-target",
    )
    .await;
    approve_pair(
        &store,
        "acme",
        "prod",
        "unrelated-bearer-value",
        "register-prod",
    )
    .await;
    let paired = store
        .request_registration_with_intent(
            target.to_owned(),
            String::from("vault"),
            registration_fingerprint("register-pending", target, "vault"),
        )
        .await
        .unwrap();
    assert!(matches!(paired, RegistrationApply::Decided(_)));
    // Two paired devices; one descriptor carries the target.
    let device = store
        .request_pairing(format!("{target} phone"), String::from("conn-1"))
        .await
        .unwrap();
    assert!(
        store
            .approve_pending(&device.pending_id, "conn-1")
            .await
            .unwrap()
            .is_some()
    );
    let other_device = store
        .request_pairing(String::from("laptop"), String::from("conn-2"))
        .await
        .unwrap();
    assert!(
        store
            .approve_pending(&other_device.pending_id, "conn-2")
            .await
            .unwrap()
            .is_some()
    );
    // Protected values are provisioned for both refs; the erasure must never
    // read, rotate, or delete them (K-C).
    let target_ref = CredentialRef::new("acme", target).unwrap();
    let unrelated_ref = CredentialRef::new("acme", "prod").unwrap();
    memory.insert(target_ref.clone(), "target-bearer-value");
    memory.insert(unrelated_ref.clone(), "unrelated-bearer-value");
    let revision_before = store.current_set_revision().await.unwrap();

    let current = super::preservation::admit(&store, target, vec![]).await;
    let applied =
        CredentialErasureRepository::erase_target_text(&store, current.condition(), target)
            .await
            .expect("the credential erasure must answer");
    let CredentialErasureOutcome::Applied { erased, remainder } = applied else {
        panic!("the current condition must apply, got {applied:?}");
    };
    assert_eq!(remainder, 0, "no credential metadata carries the target");
    assert_eq!(
        erased, 3,
        "the usable ref, the pending registration, and the paired device"
    );

    // The usable set changed: the revision advanced once, and the local
    // protected values are untouched — no external revoke or rotation exists
    // on this path, and the bearer never enters the erasure.
    assert_eq!(
        store.current_set_revision().await.unwrap().as_u64(),
        revision_before.as_u64() + 1
    );
    assert!(memory.contains(&target_ref));
    assert!(memory.contains(&unrelated_ref));
    assert_eq!(
        available_credential("acme", &target_ref.id(), &store, &memory)
            .await
            .unwrap(),
        None,
        "a deleted local ref is no longer usable"
    );
    assert!(
        available_credential("acme", &unrelated_ref.id(), &store, &memory)
            .await
            .unwrap()
            .is_some(),
        "an unrelated credential stays available"
    );
    let refs = store.list_refs().await.unwrap();
    assert_eq!(refs, vec![unrelated_ref.clone()]);

    // Duplicate sweep: nothing left to erase, no second revision bump.
    let again = CredentialErasureRepository::erase_target_text(&store, current.condition(), target)
        .await
        .unwrap();
    assert_eq!(
        again,
        CredentialErasureOutcome::Applied {
            erased: 0,
            remainder: 0
        }
    );
    assert_eq!(
        store.current_set_revision().await.unwrap().as_u64(),
        revision_before.as_u64() + 1
    );
}

/// Drives one companion to `Present` on `client` through the production
/// transition path, so the attribution and the relocation hint are real.
async fn present_on(store: &Store, companion: CompanionId, client: ClientId) {
    let attribution = store
        .load_attribution(companion.as_raw())
        .await
        .unwrap()
        .expect("the seeded attribution exists");
    let decision = store
        .compare_and_begin_transition(
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation: attribution.generation,
                expected_state: attribution.state,
                expected_active: attribution.active_client,
            },
            Some(client),
            ThinMoveReason::InitialAttach,
        )
        .await
        .unwrap();
    let MoveDecision::TransitioningToNew { generation } = decision else {
        panic!("the transition begin must apply, got {decision:?}");
    };
    let confirmed = store
        .confirm_transition(
            companion.as_raw(),
            generation,
            LiveReachabilityRef {
                client,
                connection_live: true,
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(
            confirmed,
            ConfirmTransitionOutcome::Confirmed(PresenceAttribution {
                state: PresenceState::Present,
                active_client: Some(crowned),
                ..
            }) if crowned == client
        ),
        "the fixture must be Present on the client, got {confirmed:?}"
    );
}

/// Inserts one unrelated companion with a live presence row directly: no
/// production API creates a second companion, and the erasure test needs a
/// genuinely unrelated row it can compare byte-for-byte.
fn seed_unrelated_companion(store: &Store, companion: RawId, client: ClientId) {
    let conn = store.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO companion (companion_id, lifecycle, created_at) VALUES (?1, 'running', '2026-09-17T00:00:00+09:00')",
        params![crate::codec::encode_id(companion)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO presence_attribution (companion_id, state, active_client, generation) VALUES (?1, 'present', ?2, 2)",
        params![
            crate::codec::encode_id(companion),
            crate::codec::encode_id(client.as_raw())
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO relocation_hint (companion_id, last_client, recovery_destination) VALUES (?1, ?2, NULL)",
        params![
            crate::codec::encode_id(companion),
            crate::codec::encode_id(client.as_raw())
        ],
    )
    .unwrap();
}

async fn attribution_of(store: &Store, companion: RawId) -> PresenceAttribution {
    store
        .load_attribution(companion)
        .await
        .unwrap()
        .expect("the attribution exists")
}

#[tokio::test]
async fn presence_erasure_clears_target_bearing_references_and_proves_absence() {
    let store = open_memory().await.unwrap();
    let companion = store.ensure_running_companion().await.unwrap();
    let target_client = ClientId::generate();
    let other_client = ClientId::generate();
    let other_companion = RawId::new();
    present_on(&store, companion, target_client).await;
    seed_unrelated_companion(&store, other_companion, other_client);
    let target = target_client.as_raw().as_uuid().to_string();
    let unrelated_before = attribution_of(&store, other_companion).await;
    let unrelated_hint_before = store
        .load_hint(other_companion)
        .await
        .unwrap()
        .expect("the unrelated hint exists");

    let current = super::preservation::admit(&store, &target, vec![]).await;
    let applied =
        PresenceErasureRepository::erase_target_text(&store, current.condition(), &target)
            .await
            .expect("the presence erasure must answer");
    let PresenceErasureOutcome::Applied { erased, remainder } = applied else {
        panic!("the current condition must apply, got {applied:?}");
    };
    assert_eq!(remainder, 0, "no presence-owned value names the target");
    assert_eq!(erased, 2, "the attribution stop and the hint clear");

    // The erased client can never be re-crowned: the attribution is stopped
    // with the reference cleared, and the hint keeps no copy. The unrelated
    // companion's presence is untouched.
    let attribution = store
        .load_attribution(companion.as_raw())
        .await
        .unwrap()
        .expect("the companion keeps an attribution");
    assert_eq!(attribution.state, PresenceState::Stopped);
    assert_eq!(attribution.active_client, None);
    let hint = store.load_hint(companion.as_raw()).await.unwrap().unwrap();
    assert_eq!(hint.last_client, None);
    assert_eq!(hint.recovery_destination, None);
    assert_eq!(
        attribution_of(&store, other_companion).await,
        unrelated_before
    );
    assert_eq!(
        store.load_hint(other_companion).await.unwrap(),
        Some(unrelated_hint_before)
    );

    // A duplicate pass is a no-op: the bounded verification found nothing.
    assert_eq!(
        PresenceErasureRepository::erase_target_text(&store, current.condition(), &target)
            .await
            .unwrap(),
        PresenceErasureOutcome::Applied {
            erased: 0,
            remainder: 0
        }
    );
}
