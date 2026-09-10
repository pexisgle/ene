//! `Stage 2` setup management inlet: register, assign, complete, show.
//!
//! The Client expresses setup intent and reads filtered views; every
//! acceptance happens Host-side here. `HostHandle::apply_intent` maps one
//! [`ManagementIntent`] to a domain act, and `HostHandle::answer_view` (plus
//! the `setup:show` intent target) renders the filtered [`ManagementView`].
//!
//! `Stage 2` setup-target mini-language (matched on
//! `(intent kind, target string)`; anything else answers
//! [`NeedsClarification`](ene_api::v1::management::ManagementOutcome::NeedsClarification),
//! which also covers every non-setup kind — companion shutdown, deletion,
//! tasks, schedules, rules, devices, backups — as "not in `Stage 2` scope"):
//!
//! - `(ConfigureCredentialIntent, "credential:{provider}:{label}")` records a
//!   pending credential approval through the credential owner and answers
//!   [`HeldByOperation`](ene_api::v1::management::ManagementOutcome::HeldByOperation);
//!   once the Owner approves the pair on the Host-local trusted inlet, a fresh
//!   intent observing the now-usable pair answers
//!   [`AppliedAsOneTime`](ene_api::v1::management::ManagementOutcome::AppliedAsOneTime).
//!   The bearer itself travels the Host-local protected path only: with the
//!   environment store that means the process environment, which the view
//!   below notes as env-sourced.
//! - `(ManageRuleConsentCap, "consent:{provider}:{model}:{credential-id}")`
//!   assigns the route after verifying the credential is present (the
//!   registry knows that id under the same provider and the store holds its
//!   bearer), then commits consent through the consent owner at
//!   previous-plus-one and answers
//!   [`StoredAsRuleView`](ene_api::v1::management::ManagementOutcome::StoredAsRuleView)
//!   carrying the new revision mark. Revision exhaustion clarifies; it never
//!   reuses the maximum revision with new content.
//! - `(ManageRuleConsentCap, "setup:complete")` verifies consent plus
//!   credential presence and answers `AppliedAsOneTime`. Setup completion is
//!   derived thereafter (consent stored and credential present), never written
//!   as a flag.
//! - `(ManageRuleConsentCap, "setup:show")` answers a [`ManagementView`]
//!   instead of an outcome.
//!
//! Target parsing uses the shared `ene-api` setup grammar
//! ([`parse_credential_target`],
//! [`parse_consent_target`]):
//! the Host parse is authoritative and builders never bypass validation. The
//! `NeedsClarification` DTO carries no detail string, so the "not in
//! `Stage 2` scope" note lives here in documentation, not on the wire.
//! Consent writes compare-and-save against the expectation parsed from the
//! intent `base_view`; mismatch answers
//! [`StaleBaseView`](ene_api::v1::management::ManagementOutcome::StaleBaseView)
//! with the rebuilt current mark. Registration takes no mark (no revision is
//! involved) and views are reads.
//!
//! Views never carry secrets: sections report provider, model, consent
//! revision, and credential presence only. A store failure behind a view
//! answers zero sections under the `"unavailable"` mark (documented gap: there
//! is no error DTO on the view path).
//!
//! Rationale is fingerprint material only: the inlet never acts on the
//! intent `rationale`, but its origin and quote ride the replay fingerprint
//! so a reused id with a new rationale counts as different content.
//! Assignment parameters come from the parsed consent target; the Host never
//! sends intents, so no `quote` handling exists Host-side beyond carrying it.

use ene_api::v1::management::{
    ManagementIntent, ManagementIntentKind, ManagementOutcome, ManagementView,
    ManagementViewRequest, RationaleOrigin, SETUP_COMPLETE_TARGET, SETUP_SHOW_TARGET, ViewSection,
    parse_consent_target, parse_credential_target,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::ViewMarkWire;
use ene_credential::{
    CredentialIntentRepository, RegistrationApply, RegistrationFingerprint, RegistrationState,
    available_credential,
};
use ene_permission::{
    AssignConsentIntent, AssignConsentResolution, ConsentRepository, IntentFingerprint,
    IntentOutcome, IntentOutcomeRecord, IntentOutcomeRepository, IntentResolution, assign_consent,
    consent_mark_rev,
};
use ene_plugin_ipc::WireFrame;

use crate::serve::{CredStore, HostHandle, LiveInput, outgoing_envelope, outgoing_frame};

/// Builds a management outcome reply, echoing the intent id.
///
/// The outcome is the ack of the intent saga, so the envelope carries the
/// intent id as `command_id` alongside the `reply_to` link.
fn outcome_frame(
    frame: &WireFrame,
    live: &LiveInput,
    intent: &ManagementIntent,
    outcome: ManagementOutcome,
) -> WireFrame {
    let payload = WirePayload::ManagementOutcome(outcome);
    let mut envelope = outgoing_envelope(frame, live, &payload, Some(frame.envelope.message_id));
    envelope.correlation.command_id = Some(intent.intent_id);
    WireFrame { envelope, payload }
}

/// Builds a management view reply.
fn view_frame(frame: &WireFrame, live: &LiveInput, view: ManagementView) -> WireFrame {
    outgoing_frame(frame, live, WirePayload::ManagementView(view))
}

impl HostHandle {
    /// Maps one [`ManagementIntent`] to its `Stage 2` outcome frames.
    ///
    /// Setup pairs route through the register/assign/complete/show paths
    /// below; every other kind answers `NeedsClarification` (deferred scope,
    /// never a silent accept). Store failures behind a write answer
    /// [`HeldByOperation`](ene_api::v1::management::ManagementOutcome::HeldByOperation):
    /// nothing was decided, so a later retry is safe.
    pub(crate) async fn apply_intent(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        match intent.kind {
            ManagementIntentKind::ConfigureCredentialIntent => {
                self.register_credential(frame, intent, live).await
            }
            ManagementIntentKind::ManageRuleConsentCap => {
                self.apply_consent_or_setup(frame, intent, live).await
            }
            // Deferred scope answers clarify — recorded like any other
            // decided outcome, so a retried id observes one answer.
            _ => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    self.record_decided(
                        intent,
                        Self::intent_kind_name(intent.kind),
                        IntentOutcome::NeedsClarification,
                    )
                    .await,
                )];
            }
        }
    }

    /// Renders a wire intent kind into its replay-fingerprint discriminator.
    ///
    /// Explicit match, never derived: the stored text must stay stable
    /// across refactors that do not change the wire.
    fn intent_kind_name(kind: ManagementIntentKind) -> &'static str {
        match kind {
            ManagementIntentKind::StopCompanion => "stop-companion",
            ManagementIntentKind::DeleteCompanion => "delete-companion",
            ManagementIntentKind::CancelTask => "cancel-task",
            ManagementIntentKind::ManageSchedule => "manage-schedule",
            ManagementIntentKind::DenyOrRefuse => "deny-or-refuse",
            ManagementIntentKind::ManageRuleConsentCap => "manage-rule-consent-cap",
            ManagementIntentKind::ManageDevice => "manage-device",
            ManagementIntentKind::ConfigureCredentialIntent => "configure-credential",
            ManagementIntentKind::RequestDeletionBackupRestoreReset => {
                "deletion-backup-restore-reset"
            }
        }
    }

    /// Registers the credential ref named by a `credential:` target.
    /// Registers a credential ref request through the approval gate.
    ///
    /// The wire intent only PROPOSES: it records a pending approval and
    /// answers [`HeldByOperation`](ene_api::v1::management::ManagementOutcome::HeldByOperation)
    /// until a Host-local `approve-credential` flips it usable — observed
    /// through a NEW intent id, which decides `AppliedAsOneTime` from
    /// current state. An exact retry (same id, same fingerprint) replays
    /// its stored snapshot instead: still `Held` even after approval
    /// landed elsewhere, because a new judgment requires a new key (same
    /// rule as command keys). Credential registration is high-privilege
    /// (trusted confirmation required), so the wire never creates usable
    /// refs directly.
    async fn register_credential(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        let Some((provider, label)) = parse_credential_target(&intent.target) else {
            return vec![outcome_frame(
                frame,
                live,
                intent,
                self.record_decided(
                    intent,
                    Self::INTENT_KIND_REGISTER,
                    IntentOutcome::NeedsClarification,
                )
                .await,
            )];
        };
        // Durable replay first, same contract as assign: exact retry replays
        // the stored snapshot (a held registration stays held until a NEW
        // intent observes the approval — the snapshot never goes stale by
        // itself, it just stops being the whole story once the Owner acts).
        let intent_key = intent.intent_id.0.as_hyphenated().to_string();
        match self.store.lookup_intent_outcome(&intent_key).await {
            Err(_) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::HeldByOperation,
                )];
            }
            Ok(Some(stored))
                if Self::intent_matches(&stored, intent, Self::INTENT_KIND_REGISTER) =>
            {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    Self::replayed_outcome(&stored.outcome),
                )];
            }
            Ok(Some(_)) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::NeedsClarification,
                )];
            }
            Ok(None) => {}
        }
        // One durable determination owned by `ene-credential`: the pending
        // insert (or usable recheck) and the replay row share a transaction,
        // so the snapshot and the state it describes can never strand
        // apart. A raced insert is resolved from the journal below.
        let fingerprint = Self::intent_fingerprint(intent, Self::INTENT_KIND_REGISTER);
        let registration = RegistrationFingerprint {
            intent_id: fingerprint.intent_id.clone(),
            kind: fingerprint.kind.clone(),
            target: fingerprint.target.clone(),
            base: fingerprint.base.clone(),
            rationale_origin: fingerprint.rationale_origin.clone(),
            rationale_quote: fingerprint.rationale_quote.clone(),
        };
        match self
            .store
            .request_registration_with_intent(provider, label, registration)
            .await
        {
            Ok(RegistrationApply::Decided(RegistrationState::AppliedAsOneTime)) => {
                vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::AppliedAsOneTime,
                )]
            }
            Ok(RegistrationApply::Decided(RegistrationState::HeldByOperation)) => {
                vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::HeldByOperation,
                )]
            }
            Ok(RegistrationApply::AlreadyDecided) => {
                // Lost a cross-process race: answer from the journal winner.
                match self.store.lookup_intent_outcome(&intent_key).await {
                    Ok(Some(stored)) if stored.fingerprint == fingerprint => vec![outcome_frame(
                        frame,
                        live,
                        intent,
                        Self::replayed_outcome(&stored.outcome),
                    )],
                    Ok(Some(_)) => vec![outcome_frame(
                        frame,
                        live,
                        intent,
                        ManagementOutcome::NeedsClarification,
                    )],
                    Ok(None) | Err(_) => vec![outcome_frame(
                        frame,
                        live,
                        intent,
                        ManagementOutcome::HeldByOperation,
                    )],
                }
            }
            Err(_) => vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::HeldByOperation,
            )],
        }
    }

    /// Intent kind discriminators for replay fingerprints. One per recording
    /// path; a reused id across kinds is different content by construction.
    const INTENT_KIND_ASSIGN: &str = "assign";
    const INTENT_KIND_REGISTER: &str = "register";
    const INTENT_KIND_COMPLETE: &str = "complete";

    /// Builds the replay fingerprint for `intent` under `kind`.
    fn intent_fingerprint(intent: &ManagementIntent, kind: &str) -> IntentFingerprint {
        IntentFingerprint {
            intent_id: intent.intent_id.0.as_hyphenated().to_string(),
            kind: kind.to_string(),
            target: intent.target.0.clone(),
            base: intent.base_view.0.clone(),
            rationale_origin: match intent.rationale.origin {
                RationaleOrigin::Conversation => String::from("conversation"),
                RationaleOrigin::ManagementSurface => String::from("management-surface"),
            },
            rationale_quote: intent.rationale.quote.clone(),
        }
    }

    /// Whether a stored row carries the same intent (fingerprint match; the
    /// recorded outcome is irrelevant to identity).
    fn intent_matches(stored: &IntentOutcomeRecord, intent: &ManagementIntent, kind: &str) -> bool {
        stored.fingerprint.kind == kind
            && stored.fingerprint.target == intent.target.0
            && stored.fingerprint.base == intent.base_view.0
            && stored.fingerprint.rationale_origin
                == match intent.rationale.origin {
                    RationaleOrigin::Conversation => "conversation",
                    RationaleOrigin::ManagementSurface => "management-surface",
                }
            && stored.fingerprint.rationale_quote == intent.rationale.quote
    }

    /// Maps a replayed snapshot to its answer, verbatim.
    fn replayed_outcome(snapshot: &IntentOutcome) -> ManagementOutcome {
        match snapshot {
            IntentOutcome::StoredAsRuleView { revision } => ManagementOutcome::StoredAsRuleView {
                revision: ViewMarkWire(revision.clone()),
            },
            IntentOutcome::AppliedAsOneTime => ManagementOutcome::AppliedAsOneTime,
            IntentOutcome::HeldByOperation => ManagementOutcome::HeldByOperation,
            // The wire has no exhaustion outcome: a consent identity that
            // cannot advance its revision needs Owner intervention, the same
            // answer class as any other undecidable premise.
            IntentOutcome::NeedsClarification | IntentOutcome::RevisionExhausted => {
                ManagementOutcome::NeedsClarification
            }
            IntentOutcome::StaleBaseView { current } => ManagementOutcome::StaleBaseView {
                current: ViewMarkWire(current.clone()),
            },
        }
    }

    /// Routes a consent-scope intent to assign, complete, or show.
    async fn apply_consent_or_setup(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        let target = intent.target.0.as_str();
        if target == SETUP_SHOW_TARGET {
            let view = self.build_view(&[]).await;
            return vec![view_frame(frame, live, view)];
        }
        if target == SETUP_COMPLETE_TARGET {
            return self.complete_setup(frame, intent, live).await;
        }
        let Some((provider, model, credential_id)) = parse_consent_target(&intent.target) else {
            // Malformed targets decide Clarify like any other outcome: the
            // row closes the hole where a retry could otherwise swap in a
            // valid target under the same id and reach assign. Recorded
            // under the assign kind so the fingerprint stays comparable.
            return vec![outcome_frame(
                frame,
                live,
                intent,
                self.record_decided(
                    intent,
                    Self::INTENT_KIND_ASSIGN,
                    IntentOutcome::NeedsClarification,
                )
                .await,
            )];
        };
        self.assign_consent(frame, intent, &provider, &model, &credential_id, live)
            .await
    }

    /// Stores a decided snapshot write-once and returns the wire answer.
    ///
    /// Durable-before-visible: a store failure answers
    /// [`HeldByOperation`](ene_api::v1::management::ManagementOutcome::HeldByOperation)
    /// rather than the decided outcome, so an id never observes an answer
    /// its retry cannot reproduce. A lost write race answers the winner
    /// (replay) or clarifies (conflict) — never the locally decided
    /// outcome.
    async fn record_decided(
        &self,
        intent: &ManagementIntent,
        kind: &str,
        outcome: IntentOutcome,
    ) -> ManagementOutcome {
        let answer = Self::replayed_outcome(&outcome);
        let record = IntentOutcomeRecord {
            fingerprint: Self::intent_fingerprint(intent, kind),
            outcome,
        };
        match self.store.record_intent_outcome(record).await {
            Ok(IntentResolution::Decided(())) => answer,
            Ok(IntentResolution::Replay(stored)) => Self::replayed_outcome(&stored.outcome),
            Ok(IntentResolution::Conflict(_)) => ManagementOutcome::NeedsClarification,
            Err(_) => ManagementOutcome::HeldByOperation,
        }
    }

    /// Assigns the consent route after verifying credential presence.
    ///
    /// Parses the intent `base_view` into a compare-and-save expectation
    /// against the loaded current record, then requires the credential to be
    /// both registered and bearer-present. The saved record keeps the stored
    /// id when one exists and bumps its revision by one, saturating. A lost
    /// compare race (or a view that moved between read and write) answers
    /// `StaleBaseView` with the rebuilt current mark instead of overwriting:
    /// the caller re-reads and retries.
    async fn assign_consent(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        provider: &str,
        model: &str,
        credential_id: &str,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        // Durable intent replay first (§18.2): the stored snapshot precedes
        // every premise read, so a past-success exact retry reaches its prior
        // outcome even after credential state moved on. A hit with the same
        // fingerprint answers the prior outcome verbatim (never re-executed);
        // a hit with different content clarifies instead of adopting the new
        // meaning. A miss falls through to the owner-side assignment.
        let intent_key = intent.intent_id.0.as_hyphenated().to_string();
        match self.store.lookup_intent_outcome(&intent_key).await {
            Err(_) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::HeldByOperation,
                )];
            }
            Ok(Some(stored)) if Self::intent_matches(&stored, intent, Self::INTENT_KIND_ASSIGN) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    Self::replayed_outcome(&stored.outcome),
                )];
            }
            Ok(Some(_)) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::NeedsClarification,
                )];
            }
            Ok(None) => {}
        }
        // Credential availability is a credential-owned premise: the Host
        // only crosses owners, it never combines their judgments.
        let credential_present = match available_credential(
            provider,
            credential_id,
            &self.store,
            &self.cred_store,
        )
        .await
        {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(_) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::HeldByOperation,
                )];
            }
        };
        // The consent owner decides the route: mark parsing, stale faces,
        // same-route shortcut, revision bump, and the atomic commit.
        let premises = AssignConsentIntent {
            provider: provider.to_string(),
            model: model.to_string(),
            credential_id: credential_id.to_string(),
            base_view: intent.base_view.0.clone(),
            credential_present,
            fingerprint: Self::intent_fingerprint(intent, Self::INTENT_KIND_ASSIGN),
        };
        match assign_consent(&self.store, &self.store, premises).await {
            Ok(
                AssignConsentResolution::Decided(outcome)
                | AssignConsentResolution::Replay(IntentOutcomeRecord { outcome, .. }),
            ) => vec![outcome_frame(
                frame,
                live,
                intent,
                Self::replayed_outcome(&outcome),
            )],
            Ok(AssignConsentResolution::Conflict(_)) => vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::NeedsClarification,
            )],
            Err(_) => vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::HeldByOperation,
            )],
        }
    }

    /// Completes setup after verifying the full premise, writing no flag.
    ///
    /// `AppliedAsOneTime` means consent is stored and its bearer is present
    /// right now; completion stays derived from those two facts afterwards.
    /// A stale `base_view` answers `StaleBaseView`, an unreadable store holds,
    /// and an incomplete premise clarifies.
    async fn complete_setup(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        // Durable replay first (1-a): the stored snapshot precedes any
        // currentness check, so an exact retry replays its prior outcome
        // even after the base moved on.
        let intent_key = intent.intent_id.0.as_hyphenated().to_string();
        match self.store.lookup_intent_outcome(&intent_key).await {
            Err(_) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::HeldByOperation,
                )];
            }
            Ok(Some(stored))
                if Self::intent_matches(&stored, intent, Self::INTENT_KIND_COMPLETE) =>
            {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    Self::replayed_outcome(&stored.outcome),
                )];
            }
            Ok(Some(_)) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::NeedsClarification,
                )];
            }
            Ok(None) => {}
        }
        // Bearer premise for the atomic claim below: the credential owner
        // resolves registered-and-backed availability; the transaction
        // decides completion. Unreadable stores hold, as before.
        let bearer_present = match self.store.load_current().await {
            Err(_) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::HeldByOperation,
                )];
            }
            Ok(None) => false,
            Ok(Some(consent)) => {
                match available_credential(
                    &consent.provider,
                    &consent.credential_id,
                    &self.store,
                    &self.cred_store,
                )
                .await
                {
                    Ok(found) => found.is_some(),
                    Err(_) => {
                        return vec![outcome_frame(
                            frame,
                            live,
                            intent,
                            ManagementOutcome::HeldByOperation,
                        )];
                    }
                }
            }
        };
        // One durable determination (1-b): compare, completability, and
        // snapshot-save share a transaction; the answer below renders the
        // decided snapshot verbatim. A raced claim answers from the winner.
        match self
            .store
            .complete_with_intent(
                intent.base_view.0.clone(),
                bearer_present,
                Self::intent_fingerprint(intent, Self::INTENT_KIND_COMPLETE),
            )
            .await
        {
            Ok(IntentResolution::Decided(decided) | IntentResolution::Replay(decided)) => {
                vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    Self::replayed_outcome(&decided.outcome),
                )]
            }
            Ok(IntentResolution::Conflict(_)) => vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::NeedsClarification,
            )],
            Err(_) => vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::HeldByOperation,
            )],
        }
    }

    /// Answers one [`ManagementViewRequest`] with the filtered view.
    ///
    /// An empty section list selects every `Stage 2` section (`provider`,
    /// `model`, `consent`, `credential`); otherwise only requested known
    /// sections render and unknown names are skipped.
    pub(crate) async fn answer_view(
        &self,
        frame: &WireFrame,
        request: &ManagementViewRequest,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        let view = self.build_view(&request.sections).await;
        vec![view_frame(frame, live, view)]
    }

    /// Builds the filtered setup view for the wanted sections.
    ///
    /// Bodies carry display facts only — provider, model, consent revision,
    /// credential presence plus the bearer-source note — never secrets. The
    /// mark mirrors the consent revision so consent writes can check
    /// `base_view` staleness against it.
    pub(crate) async fn build_view(&self, wanted: &[String]) -> ManagementView {
        let Ok(current) = self.store.load_current().await else {
            return unavailable_view();
        };
        let credential_present = match &current {
            Some(record) => {
                match available_credential(
                    &record.provider,
                    &record.credential_id,
                    &self.store,
                    &self.cred_store,
                )
                .await
                {
                    Ok(found) => found.is_some(),
                    Err(_) => return unavailable_view(),
                }
            }
            None => false,
        };
        let (provider_text, model_text, consent_text) = match &current {
            Some(record) => (
                record.provider.clone(),
                record.model.clone(),
                format!("rev {}", record.rev.as_u64()),
            ),
            None => (
                String::from("unconfigured"),
                String::from("unconfigured"),
                String::from("none"),
            ),
        };
        let mark_text = consent_mark_rev(current.as_ref().map(|record| record.rev.as_u64()));
        let source = match &self.cred_store {
            CredStore::Env(_) => "env-sourced",
            CredStore::Memory(_) => "memory",
        };
        let credential_text = if credential_present {
            format!("present ({source})")
        } else {
            String::from("absent")
        };
        let candidates = [
            ("provider", "Provider", provider_text),
            ("model", "Model", model_text),
            ("consent", "Consent", consent_text),
            ("credential", "Credential", credential_text),
        ];
        let sections = candidates
            .into_iter()
            .filter(|(kind, _, _)| wanted.is_empty() || wanted.iter().any(|name| name == kind))
            .map(|(kind, title, body)| ViewSection {
                kind: kind.to_string(),
                title: title.to_string(),
                body,
            })
            .collect();
        ManagementView {
            mark: ViewMarkWire(mark_text),
            sections,
        }
    }
}

/// Builds the view answering a store failure: no sections, pinned mark.
fn unavailable_view() -> ManagementView {
    ManagementView {
        mark: ViewMarkWire(String::from("unavailable")),
        sections: Vec::new(),
    }
}
