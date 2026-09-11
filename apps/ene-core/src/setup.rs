//! Host management inlet: setup intents, the setup view, and the read-only
//! Memory view.
//!
//! The Client expresses setup intent and reads filtered views; every
//! acceptance happens Host-side here. Memory has no write path: corrections
//! and changes arrive as Experience through Learning, never by editing a
//! canonical row.
//!
//! Setup targets are matched on `(intent kind, target string)`; anything else
//! — every non-setup kind included — answers
//! [`NeedsClarification`](ene_api::v1::management::ManagementOutcome::NeedsClarification)
//! as "not in `Stage 2` scope":
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
//! is no error DTO on the view path). A request that names only `memory` reads
//! no setup state, so it still renders while consent or credential state is
//! unreadable; its mark stays `"unavailable"` because it named no management
//! revision to build on.
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
use ene_companion::CompanionRepository;
use ene_credential::{
    CredentialIntentRepository, RegistrationApply, RegistrationFingerprint, RegistrationState,
    available_credential,
};
use ene_learning::{
    ChangeKind, LearningRepository, Memory, MemoryId, MemoryRevision, MemoryRevisionRecord,
    SummaryId, TemporalMeaning,
};
use ene_permission::{
    AssignConsentIntent, CapabilityKind, ConsentRecord, ConsentRepository, IntentFingerprint,
    IntentOutcome, IntentOutcomeRecord, IntentOutcomeRepository, IntentResolution, assign_consent,
    consent_view_mark,
};
use ene_plugin_ipc::WireFrame;
use ene_primitive::RawId;

use crate::serve::{CredStore, HostHandle, LiveInput, outgoing_envelope, outgoing_frame};

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

fn view_frame(frame: &WireFrame, live: &LiveInput, view: ManagementView) -> WireFrame {
    outgoing_frame(frame, live, WirePayload::ManagementView(view))
}

/// One parsed consent assignment target: capability plus route.
///
/// Keeps the assignment parameters grouped so the capability can never be
/// separated from the route it authorizes at the call boundary.
struct ConsentTarget {
    capability: CapabilityKind,
    provider: String,
    model: String,
    credential_id: String,
}

impl HostHandle {
    /// Every other kind answers `NeedsClarification` (deferred scope, never a
    /// silent accept). Store failures behind a write answer
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
            // Deferred scope answers clarify, recorded like any other decided
            // outcome so a retried id observes one answer.
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

    /// The wire intent only PROPOSES: a held snapshot stays held until a NEW
    /// intent id observes the approval, because a new judgment requires a new
    /// key (same rule as command keys). Credential registration is
    /// high-privilege (trusted confirmation required), so the wire never
    /// creates usable refs directly.
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
        // Durable replay first: an exact retry replays its stored snapshot.
        if let Some(answer) = self
            .replay_or_hold(frame, live, intent, Self::INTENT_KIND_REGISTER)
            .await
        {
            return answer;
        }
        // One durable determination owned by `ene-credential`: the pending
        // insert (or usable recheck) and the replay row share a transaction,
        // so snapshot and state can never strand apart. A raced insert is
        // resolved from the journal below.
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
                let intent_key = intent.intent_id.0.as_hyphenated().to_string();
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

    /// Replay-fingerprint discriminators, one per recording path; a reused id
    /// across kinds is different content by construction.
    const INTENT_KIND_ASSIGN: &str = "assign";
    const INTENT_KIND_REGISTER: &str = "register";
    const INTENT_KIND_COMPLETE: &str = "complete";

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

    /// Identity is the fingerprint only; the recorded outcome is irrelevant.
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

    /// Durable intent replay first (§18.2): the stored snapshot precedes
    /// every premise read, so a past-success exact retry reaches its prior
    /// outcome even after state moved on.
    ///
    /// A hit with the same fingerprint answers verbatim (never re-executed);
    /// a hit with different content clarifies instead of adopting the new
    /// meaning; an unreadable journal holds. A miss returns [`None`] so the
    /// caller falls through to the owner-side execution.
    async fn replay_or_hold(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        intent: &ManagementIntent,
        kind: &str,
    ) -> Option<Vec<WireFrame>> {
        let intent_key = intent.intent_id.0.as_hyphenated().to_string();
        match self.store.lookup_intent_outcome(&intent_key).await {
            Err(_) => Some(vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::HeldByOperation,
            )]),
            Ok(Some(stored)) if Self::intent_matches(&stored, intent, kind) => {
                Some(vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    Self::replayed_outcome(&stored.outcome),
                )])
            }
            Ok(Some(_)) => Some(vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::NeedsClarification,
            )]),
            Ok(None) => None,
        }
    }

    async fn apply_consent_or_setup(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        let target = intent.target.0.as_str();
        if target == SETUP_SHOW_TARGET {
            let view = self.build_view(&[], None, None, None).await;
            return vec![view_frame(frame, live, view)];
        }
        if target == SETUP_COMPLETE_TARGET {
            return self.complete_setup(frame, intent, live).await;
        }
        let Some((capability, provider, model, credential_id)) =
            parse_consent_target(&intent.target)
        else {
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
        let Some(capability) = CapabilityKind::from_name(&capability) else {
            // Unknown capabilities are outside the closed world; a retry
            // under the same id observes the same clarification.
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
        let target = ConsentTarget {
            capability,
            provider,
            model,
            credential_id,
        };
        self.assign_consent(frame, intent, &target, live).await
    }

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

    /// The saved record keeps the stored id when one exists and advances its
    /// revision. A lost compare race answers `StaleBaseView` with the rebuilt
    /// current mark instead of overwriting: the caller re-reads and retries.
    async fn assign_consent(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        target: &ConsentTarget,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        let ConsentTarget {
            capability,
            provider,
            model,
            credential_id,
        } = target;
        // Durable intent replay first (§18.2): a past-success exact retry
        // reaches its prior outcome; a miss falls through to the owner-side
        // assignment.
        if let Some(answer) = self
            .replay_or_hold(frame, live, intent, Self::INTENT_KIND_ASSIGN)
            .await
        {
            return answer;
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
        // same-route shortcut, revision bump, and the atomic commit, all
        // scoped to the capability the target names.
        let premises = AssignConsentIntent {
            capability: *capability,
            provider: provider.to_string(),
            model: model.to_string(),
            credential_id: credential_id.to_string(),
            base_view: intent.base_view.0.clone(),
            credential_present,
            fingerprint: Self::intent_fingerprint(intent, Self::INTENT_KIND_ASSIGN),
        };
        match assign_consent(&self.store, &self.store, premises).await {
            Ok(
                IntentResolution::Decided(outcome)
                | IntentResolution::Replay(IntentOutcomeRecord { outcome, .. }),
            ) => vec![outcome_frame(
                frame,
                live,
                intent,
                Self::replayed_outcome(&outcome),
            )],
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

    async fn complete_setup(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        // Durable replay first: the stored snapshot precedes any currentness
        // check, so an exact retry replays its prior outcome.
        if let Some(answer) = self
            .replay_or_hold(frame, live, intent, Self::INTENT_KIND_COMPLETE)
            .await
        {
            return answer;
        }
        // Bearer premise for the atomic claim below: the credential owner
        // resolves registered-and-backed availability; the transaction
        // decides completion. Unreadable stores hold.
        let bearer_present = match self.store.load_current(CapabilityKind::Dialogue).await {
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
        // One durable determination: compare, completability, and
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

    /// An empty section list selects every known section (`provider`,
    /// `model`, `consent`, `credential`, `memory`); otherwise only requested
    /// known sections render and unknown names are skipped. `memory_after`
    /// continues the current-memory list; `memory_revisions_of` (with
    /// `memory_revisions_after`) renders one Memory's revision page instead.
    pub(crate) async fn answer_view(
        &self,
        frame: &WireFrame,
        request: &ManagementViewRequest,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        let view = self
            .build_view(
                &request.sections,
                request.memory_after.as_deref(),
                request.memory_revisions_of.as_deref(),
                request.memory_revisions_after,
            )
            .await;
        vec![view_frame(frame, live, view)]
    }

    /// Bodies carry display facts only — provider, model, consent revision,
    /// credential presence plus the bearer-source note — never secrets. Each
    /// capability owns its section and consent revision; the mark carries
    /// both segments so a consent write is checked against the capability it
    /// names. Setup state is read only for sections that report it; a
    /// `memory`-only request renders Memory without touching the consent or
    /// credential stores.
    pub(crate) async fn build_view(
        &self,
        wanted: &[String],
        memory_after: Option<&str>,
        memory_revisions_of: Option<&str>,
        memory_revisions_after: Option<u64>,
    ) -> ManagementView {
        let wants = |name: &str| wanted.is_empty() || wanted.iter().any(|section| section == name);
        let mut sections = Vec::new();
        // A request that names only `memory` reads no setup state, so a
        // consent or credential-store failure cannot hide the Memory section.
        // Its mark stays unavailable because no management revision was read.
        let wants_setup = wanted.is_empty() || wanted.iter().any(|name| name != "memory");
        let mark = if wants_setup {
            let dialogue = match self.store.load_current(CapabilityKind::Dialogue).await {
                Ok(record) => record,
                Err(_) => return unavailable_view(),
            };
            let learning = match self.store.load_current(CapabilityKind::Learning).await {
                Ok(record) => record,
                Err(_) => return unavailable_view(),
            };
            let Some(dialogue_present) = self.credential_present(dialogue.as_ref()).await else {
                return unavailable_view();
            };
            let Some(learning_present) = self.credential_present(learning.as_ref()).await else {
                return unavailable_view();
            };
            let source = match &self.cred_store {
                CredStore::Env(_) => "env-sourced",
                CredStore::Memory(_) => "memory",
            };
            let (provider_text, model_text, consent_text) = match &dialogue {
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
            let learning_text = match &learning {
                Some(record) => format!(
                    "provider={} model={} consent=rev {} credential={}",
                    record.provider,
                    record.model,
                    record.rev.as_u64(),
                    presence_text(learning_present, source),
                ),
                None => String::from("unconfigured"),
            };
            for (kind, title, body) in [
                ("provider", "Provider", provider_text),
                ("model", "Model", model_text),
                ("consent", "Consent", consent_text),
                (
                    "credential",
                    "Credential",
                    presence_text(dialogue_present, source),
                ),
                ("learning", "Learning", learning_text),
            ] {
                if wants(kind) {
                    sections.push(ViewSection {
                        kind: kind.to_string(),
                        title: title.to_string(),
                        body,
                    });
                }
            }
            consent_view_mark(
                dialogue.as_ref().map(|record| record.rev.as_u64()),
                learning.as_ref().map(|record| record.rev.as_u64()),
            )
        } else {
            String::from("unavailable")
        };
        if wants("memory") {
            sections.push(ViewSection {
                kind: String::from("memory"),
                title: String::from("Memory"),
                body: self
                    .render_memory_view(memory_after, memory_revisions_of, memory_revisions_after)
                    .await,
            });
        }
        ManagementView {
            mark: ViewMarkWire(mark),
            sections,
        }
    }

    /// Credential presence for one consent route, `None` when the premise
    /// cannot be resolved (the caller answers an unavailable view).
    async fn credential_present(&self, record: Option<&ConsentRecord>) -> Option<bool> {
        let Some(record) = record else {
            return Some(false);
        };
        match available_credential(
            &record.provider,
            &record.credential_id,
            &self.store,
            &self.cred_store,
        )
        .await
        {
            Ok(found) => Some(found.is_some()),
            Err(_) => None,
        }
    }

    /// Read-only projection of current Memory and its change history.
    ///
    /// With no direction, renders one page of [`MEMORY_PAGE_SIZE`] current
    /// memories, newest first, continuing strictly after `after` and ending
    /// with `next: <id>` while more remain. With `revisions_of`, renders one
    /// page of that Memory's revisions (oldest first) plus their grounds,
    /// continuing strictly after `after_revision` and ending with
    /// `next-revision: <n>` while more remain. Both pages stop at a body byte
    /// budget, so a large corpus never inflates one frame past the IPC cap;
    /// the cursor is the last rendered item, so stopping early cannot skip or
    /// duplicate a row.
    ///
    /// The list and the revision detail are separate reads on purpose: a
    /// Memory with hundreds of revisions must not enlarge the list page.
    /// There is no write path here: corrections and changes arrive as
    /// Experience through Learning, never by editing a Memory row.
    async fn render_memory_view(
        &self,
        after: Option<&str>,
        revisions_of: Option<&str>,
        after_revision: Option<u64>,
    ) -> String {
        match revisions_of {
            Some(raw) => self.render_revision_page(raw, after_revision).await,
            None => self.render_memory_list(after).await,
        }
    }

    async fn render_memory_list(&self, after: Option<&str>) -> String {
        let Ok(companion) = self.store.ensure_running_companion().await else {
            return String::from("unavailable");
        };
        let cursor = match after {
            Some(raw) => match parse_memory_id(raw) {
                Some(id) => Some(id),
                None => return String::from("invalid cursor"),
            },
            None => None,
        };
        let Ok(memories) = self
            .store
            .list_current_memories(companion.as_raw(), cursor, MEMORY_PAGE_SIZE + 1)
            .await
        else {
            return String::from("unavailable");
        };
        if memories.is_empty() {
            return if after.is_some() {
                String::from("no older memories")
            } else {
                String::from("(none)")
            };
        }
        let mut body = String::new();
        let mut last_id = memories[0].id;
        let mut rendered = 0_usize;
        for memory in memories.iter().take(MEMORY_PAGE_SIZE as usize) {
            let line = render_memory(memory);
            if rendered > 0 && body.len() + line.len() > MEMORY_BODY_BUDGET {
                break;
            }
            body.push_str(&line);
            body.push('\n');
            last_id = memory.id;
            rendered += 1;
        }
        if rendered < memories.len() {
            // The cursor is the last rendered id, so the next page starts at
            // the first unrendered memory and cannot skip or repeat a row.
            body.push_str(&format!("next: {}\n", last_id.as_raw().as_uuid()));
        }
        body.trim_end().to_owned()
    }

    async fn render_revision_page(&self, memory: &str, after_revision: Option<u64>) -> String {
        let Some(memory_id) = parse_memory_id(memory) else {
            return String::from("invalid cursor");
        };
        let after = match after_revision {
            Some(revision) if revision > 0 => Some(MemoryRevision::from_u64(revision)),
            _ => None,
        };
        let Ok(revisions) = self
            .store
            .list_memory_revisions(memory_id, after, MEMORY_REVISION_PAGE_SIZE + 1)
            .await
        else {
            return String::from("unavailable");
        };
        if revisions.is_empty() {
            return if after.is_some() {
                String::from("no more revisions")
            } else {
                String::from("unknown memory")
            };
        }
        // One batch lookup for the page's grounds: a shared Summary is read
        // once, and a Summary id with no stored row is rendered as its own
        // unavailable state rather than silently omitted.
        let summary_ids: Vec<SummaryId> = revisions.iter().filter_map(|r| r.summary).collect();
        let loaded = self.store.load_summaries(&summary_ids).await;
        let summaries_unavailable = loaded.is_err();
        let loaded = loaded.unwrap_or_default();
        let mut body = String::new();
        let mut last_revision = revisions[0].revision;
        let mut rendered = 0_usize;
        for revision in revisions.iter().take(MEMORY_REVISION_PAGE_SIZE as usize) {
            let mut piece = render_revision(revision);
            if let Some(summary_id) = revision.summary {
                let short = short_id(summary_id.as_raw());
                match loaded.iter().find(|summary| summary.id == summary_id) {
                    Some(summary) => {
                        piece.push_str(&format!("  grounds summary {short}: {}\n", summary.content))
                    }
                    None if summaries_unavailable => {
                        piece.push_str("  grounds: unavailable\n");
                    }
                    None => piece.push_str(&format!("  grounds summary {short}: unavailable\n")),
                }
            }
            if rendered > 0 && body.len() + piece.len() > MEMORY_BODY_BUDGET {
                break;
            }
            body.push_str(&piece);
            last_revision = revision.revision;
            rendered += 1;
        }
        if rendered < revisions.len() {
            body.push_str(&format!("next-revision: {}\n", last_revision.as_u64()));
        }
        body.trim_end().to_owned()
    }
}

fn presence_text(present: bool, source: &str) -> String {
    if present {
        format!("present ({source})")
    } else {
        String::from("absent")
    }
}

/// Current memories rendered by one management view page.
const MEMORY_PAGE_SIZE: u64 = 20;

/// Revisions rendered by one revision-history page.
const MEMORY_REVISION_PAGE_SIZE: u64 = 20;

/// Soft cap on one memory section body before frame encoding.
///
/// The IPC frame cap is 256 KiB including the envelope and every other
/// section; this leaves headroom. Both pages stop at the last item that
/// fits, so the cursor continues without skips and a large corpus can never
/// inflate one frame without bound.
const MEMORY_BODY_BUDGET: usize = 192 * 1024;

fn parse_memory_id(raw: &str) -> Option<MemoryId> {
    uuid::Uuid::parse_str(raw)
        .ok()
        .map(RawId::from_uuid)
        .map(MemoryId::from_raw)
}

fn short_id(id: RawId) -> String {
    id.as_uuid()
        .as_hyphenated()
        .to_string()
        .chars()
        .take(8)
        .collect()
}

fn render_memory(memory: &Memory) -> String {
    format!(
        "memory {} scope=companion importance={} temporal={} recall={} revision={} updated={}\ncontent: {}\n",
        memory.id.as_raw().as_uuid(),
        memory.importance.as_u8(),
        temporal_label(memory.temporal),
        if memory.recall_suppressed {
            "suppressed"
        } else {
            "active"
        },
        memory.revision.as_u64(),
        memory.updated_at.to_rfc3339(),
        memory.content,
    )
}

fn render_revision(revision: &MemoryRevisionRecord) -> String {
    format!(
        "  rev{} {} at={} content: {}\n",
        revision.revision.as_u64(),
        change_label(revision.change),
        revision.at.to_rfc3339(),
        revision.content,
    )
}

fn temporal_label(temporal: TemporalMeaning) -> &'static str {
    match temporal {
        TemporalMeaning::Enduring => "enduring",
        TemporalMeaning::Event => "event",
    }
}

fn change_label(change: ChangeKind) -> &'static str {
    match change {
        ChangeKind::Initial => "initial",
        ChangeKind::Reinforced => "reinforced",
        ChangeKind::Refined => "refined",
        ChangeKind::Integrated => "integrated",
        ChangeKind::CorrectedInitiallyWrong => "corrected-initially-wrong",
        ChangeKind::ChangedSince => "changed-since",
        ChangeKind::Forgotten => "forgotten",
    }
}

fn unavailable_view() -> ManagementView {
    ManagementView {
        mark: ViewMarkWire(String::from("unavailable")),
        sections: Vec::new(),
    }
}
