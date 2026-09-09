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
//! - `(ConfigureCredentialIntent, "credential:{provider}:{label}")` registers a
//!   credential ref through [`ene_credential::register`] (never overwriting)
//!   and answers [`AppliedAsOneTime`](ene_api::v1::management::ManagementOutcome::AppliedAsOneTime).
//!   The bearer itself travels the Host-local protected path only: with the
//!   environment store that means the process environment, which the view
//!   below notes as env-sourced.
//! - `(ManageRuleConsentCap, "consent:{provider}:{model}:{credential-id}")`
//!   assigns the route after verifying the credential is present (registry
//!   knows it and the store holds its bearer), then commits consent through
//!   compare-and-save at revision previous-plus-one and answers
//!   [`StoredAsRuleView`](ene_api::v1::management::ManagementOutcome::StoredAsRuleView)
//!   carrying the new revision mark.
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
use ene_credential::{CredentialRef, CredentialRefRepository, CredentialStore};
use ene_permission::{
    ConsentCommitOutcome, ConsentRecord, ConsentRepository, ConsentRevision, IntentFingerprint,
    IntentOutcome, IntentOutcomeRecord, IntentOutcomeRepository, IntentResolution,
    ShortcutIntentOutcome,
};
use ene_plugin_ipc::WireFrame;
use ene_primitive::RawId;

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
    let mut envelope = outgoing_envelope(
        frame,
        live,
        "ManagementOutcome",
        Some(frame.envelope.message_id),
    );
    envelope.correlation.command_id = Some(intent.intent_id);
    WireFrame {
        envelope,
        payload: WirePayload::ManagementOutcome(outcome),
    }
}

/// Builds a management view reply.
fn view_frame(frame: &WireFrame, live: &LiveInput, view: ManagementView) -> WireFrame {
    outgoing_frame(
        frame,
        live,
        "ManagementView",
        WirePayload::ManagementView(view),
    )
}

/// Renders the consent display mark for an optional stored record.
///
/// `"consent-rev-{n}"` over the stored revision, or `"consent-none"` when
/// nothing is stored. The same mark travels in views and in
/// [`StaleBaseView`](ene_api::v1::management::ManagementOutcome::StaleBaseView)
/// outcomes, so staleness checks compare against one vocabulary.
fn consent_mark(current: Option<&ConsentRecord>) -> String {
    match current {
        Some(record) => format!("consent-rev-{}", record.rev.as_u64()),
        None => String::from("consent-none"),
    }
}

/// Parsed consent `base_view` mark: a compare-and-save expectation.
///
/// Three cases, never collapsed: expecting empty, expecting a stored id at a
/// parsed revision, or stale on its face. Callers answer `StaleBaseView` with
/// the rebuilt current mark on [`ConsentExpectation::FaceStale`] instead of
/// writing.
enum ConsentExpectation {
    /// The mark (`"consent-none"`) expects no stored row.
    ExpectEmpty,
    /// The mark (`"consent-rev-N"`) expects the loaded current consent id at
    /// the parsed revision.
    ExpectRevision(String, ConsentRevision),
    /// The mark is stale on its face: unparseable, or a revision claim with
    /// no stored row.
    FaceStale,
}

/// Parses the intent `base_view` mark into a compare-and-save expectation.
///
/// `"consent-none"` expects no stored row; `"consent-rev-N"` expects the
/// loaded current consent id at revision `N`.
fn consent_expectation(base_view: &str, current: Option<&ConsentRecord>) -> ConsentExpectation {
    if base_view == "consent-none" {
        return ConsentExpectation::ExpectEmpty;
    }
    let Some(revision_text) = base_view.strip_prefix("consent-rev-") else {
        return ConsentExpectation::FaceStale;
    };
    let Ok(revision_number) = revision_text.parse::<u64>() else {
        return ConsentExpectation::FaceStale;
    };
    let Some(stored) = current else {
        return ConsentExpectation::FaceStale;
    };
    ConsentExpectation::ExpectRevision(
        stored.id.clone(),
        ConsentRevision::from_u64(revision_number),
    )
}

/// Default non-secret credential ref used before any consent exists.
///
/// The bearer behind it still comes from the held store at call time; this
/// ref only names the conventional `openai` main credential.
fn default_credential() -> CredentialRef {
    CredentialRef {
        id: String::from("openai:main"),
        provider: String::from("openai"),
        label: String::from("main"),
    }
}

impl HostHandle {
    /// Resolves the credential ref the startup transport bills against.
    ///
    /// Prefers the consent-bound ref from the registry when present, else a
    /// synthesized ref from the consent record, else the conventional default.
    /// Best-effort: store failures fall back to the default because the
    /// per-frame consent checks stay authoritative regardless.
    pub(crate) async fn startup_credential(&self) -> CredentialRef {
        let Ok(current) = self.store.load_current().await else {
            return default_credential();
        };
        let Some(consent) = current else {
            return default_credential();
        };
        match self.store.list_refs().await {
            Ok(refs) => match refs.iter().find(|known| known.id == consent.credential_id) {
                Some(known) => known.clone(),
                None => CredentialRef {
                    id: consent.credential_id.clone(),
                    provider: consent.provider.clone(),
                    label: String::from("main"),
                },
            },
            Err(_) => default_credential(),
        }
    }

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
        // One durable determination: propose-or-report plus the replay row
        // share a transaction, so the snapshot and the state it describes
        // can never strand apart. A raced insert answers from the winner.
        let fingerprint = Self::intent_fingerprint(intent, Self::INTENT_KIND_REGISTER);
        match self
            .store
            .request_approval_with_intent(provider, label, fingerprint)
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
            IntentOutcome::NeedsClarification => ManagementOutcome::NeedsClarification,
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
            return vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::NeedsClarification,
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
        // every check below — including current and credential reads — so a
        // past-success exact retry reaches its prior outcome even after
        // credential state moved on. A hit with the same fingerprint answers
        // the prior outcome verbatim (never re-executed); a hit with
        // different content clarifies instead of adopting the new meaning. A
        // miss falls through to the normal premise-checked path below.
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
        let Ok(current) = self.store.load_current().await else {
            return vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::HeldByOperation,
            )];
        };
        let expected = match consent_expectation(&intent.base_view.0, current.as_ref()) {
            ConsentExpectation::ExpectEmpty => None,
            ConsentExpectation::ExpectRevision(id, revision) => Some((id, revision)),
            ConsentExpectation::FaceStale => {
                // Decided from verified reads; the row makes the id observe
                // one answer forever (or holds when the store is down).
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    self.record_decided(
                        intent,
                        Self::INTENT_KIND_ASSIGN,
                        IntentOutcome::StaleBaseView {
                            current: consent_mark(current.as_ref()),
                        },
                    )
                    .await,
                )];
            }
        };
        let Ok(refs) = self.store.list_refs().await else {
            return vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::HeldByOperation,
            )];
        };
        let Some(credential) = refs.iter().find(|known| known.id == credential_id).cloned() else {
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
        if !self.cred_store.contains(&credential) {
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
        }
        // Idempotent retry: when the stored route already equals the
        // requested one, answer the current revision without bumping. A
        // transport retry reuses the intent id with identical content, so
        // bumping again would fork revisions for one Owner decision. The
        // base premise is enforced BEFORE the shortcut: a stale base with a
        // coincidentally equal route must answer stale (so the caller
        // reloads and converges), never silent success — otherwise a
        // different intent built on a moved base would succeed without ever
        // observing the move. (The durable replay above is the only
        // stale-base success, and only for the same intent id AND the same
        // fingerprint.) Genuine changes still flow into the atomic assign
        // below, where a moved base answers stale instead of overwriting.
        let base_fresh = match (&expected, current.as_ref()) {
            (None, None) => true,
            (Some((id, revision)), Some(record)) => record.id == *id && record.rev == *revision,
            (None, Some(_)) | (Some(_), None) => false,
        };
        if !base_fresh {
            return vec![outcome_frame(
                frame,
                live,
                intent,
                self.record_decided(
                    intent,
                    Self::INTENT_KIND_ASSIGN,
                    IntentOutcome::StaleBaseView {
                        current: consent_mark(current.as_ref()),
                    },
                )
                .await,
            )];
        }
        // Same-route shortcut through one atomic claim (1-d): read current
        // and record together, so the replay row can never strand apart
        // from the state it describes. A raced claim answers from the
        // winner instead of forking.
        match self
            .store
            .shortcut_with_intent(
                provider.to_string(),
                model.to_string(),
                credential_id.to_string(),
                Self::intent_fingerprint(intent, Self::INTENT_KIND_ASSIGN),
            )
            .await
        {
            Ok(IntentResolution::Decided(ShortcutIntentOutcome::Hit { current })) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::StoredAsRuleView {
                        revision: ViewMarkWire(current.rev.as_u64().to_string()),
                    },
                )];
            }
            Ok(IntentResolution::Decided(ShortcutIntentOutcome::Miss { .. })) => {}
            Ok(IntentResolution::Replay(stored)) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    Self::replayed_outcome(&stored.outcome),
                )];
            }
            Ok(IntentResolution::Conflict(_)) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::NeedsClarification,
                )];
            }
            Err(_) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::HeldByOperation,
                )];
            }
        }
        let next_rev = match current.as_ref() {
            Some(record) => record.rev.as_u64().saturating_add(1),
            None => 1,
        };
        let id = match current {
            Some(record) => record.id,
            None => RawId::new().as_uuid().to_string(),
        };
        let record = ConsentRecord {
            id,
            rev: ConsentRevision::from_u64(next_rev),
            provider: provider.to_string(),
            model: model.to_string(),
            credential_id: credential_id.to_string(),
        };
        match self
            .store
            .assign_with_intent(
                expected,
                record,
                Self::intent_fingerprint(intent, Self::INTENT_KIND_ASSIGN),
            )
            .await
        {
            Ok(IntentResolution::Decided(ConsentCommitOutcome::Committed { record })) => {
                vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::StoredAsRuleView {
                        revision: ViewMarkWire(record.rev.as_u64().to_string()),
                    },
                )]
            }
            Ok(IntentResolution::Decided(ConsentCommitOutcome::StaleCurrent { current })) => {
                vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::StaleBaseView {
                        current: ViewMarkWire(consent_mark(current.as_ref())),
                    },
                )]
            }
            Ok(IntentResolution::Replay(stored)) => vec![outcome_frame(
                frame,
                live,
                intent,
                Self::replayed_outcome(&stored.outcome),
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
        // Bearer premise for the atomic claim below: locate the credential
        // without deciding anything (the transaction decides). Unreadable
        // stores hold, as before.
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
            Ok(Some(consent)) => match self.store.list_refs().await {
                Err(_) => {
                    return vec![outcome_frame(
                        frame,
                        live,
                        intent,
                        ManagementOutcome::HeldByOperation,
                    )];
                }
                Ok(refs) => refs
                    .iter()
                    .find(|known| known.id == consent.credential_id)
                    .is_some_and(|credential| self.cred_store.contains(credential)),
            },
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
        let Ok(refs) = self.store.list_refs().await else {
            return unavailable_view();
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
        let mark_text = consent_mark(current.as_ref());
        let source = match &self.cred_store {
            CredStore::Env(_) => "env-sourced",
            CredStore::Memory(_) => "memory",
        };
        let credential_text = match &current {
            Some(record) => match refs.iter().find(|known| known.id == record.credential_id) {
                Some(known) if self.cred_store.contains(known) => {
                    format!("present ({source})")
                }
                _ => String::from("absent"),
            },
            None => String::from("absent"),
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
