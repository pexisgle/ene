use ene_action::WorkspaceRoot;
use ene_api::v1::management::{
    ManagementIntent, ManagementIntentKind, ManagementOutcome, ManagementView,
    ManagementViewRequest, RationaleOrigin, SETUP_COMPLETE_TARGET, SETUP_SHOW_TARGET, ViewSection,
    parse_consent_target, parse_credential_target, parse_task_target, parse_workspace_target,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::ViewMarkWire;
use ene_companion::{
    ActivityRepository as _, CompanionId, CompanionRepository, RecordResumeActivityCommand,
    ResumeActivityOutcome,
};
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
use ene_task::{
    CancelTaskCommand, ResumeInstructionSource, ResumeTaskCommand, SteeringPremiseRef,
    TaskCancelOutcome, TaskId, TaskRepository as _, TaskResumeOutcome, WorkspaceFolderRef,
};

use crate::presentation::CurrentCoverage;
use crate::serve::{CredStore, HostHandle, LiveInput, outgoing_envelope, outgoing_frame};

pub(crate) fn outcome_frame(
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

struct ConsentTarget {
    capability: CapabilityKind,
    provider: String,
    model: String,
    credential_id: String,
}

impl HostHandle {
    pub(crate) async fn apply_intent(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        if intent.confirmed {
            return vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::DeniedByBoundary,
            )];
        }
        match intent.kind {
            ManagementIntentKind::ConfigureCredentialIntent => {
                self.register_credential(frame, intent, live).await
            }
            ManagementIntentKind::ManageRuleConsentCap => {
                self.apply_consent_or_setup(frame, intent, live).await
            }
            ManagementIntentKind::CancelTask => self.cancel_task_intent(frame, intent, live).await,
            ManagementIntentKind::ResumeTask => self.resume_task_intent(frame, intent, live).await,
            ManagementIntentKind::SelectWorkspace => {
                self.select_workspace_intent(frame, intent, live).await
            }
            ManagementIntentKind::RequestDeletionBackupRestoreReset => {
                self.targeted_deletion_intent(frame, intent, live).await
            }
            _ => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    self.record_decided(
                        Self::intent_fingerprint(intent, Self::intent_kind_name(intent.kind)),
                        IntentOutcome::NeedsClarification,
                    )
                    .await,
                )];
            }
        }
    }

    fn intent_kind_name(kind: ManagementIntentKind) -> &'static str {
        match kind {
            ManagementIntentKind::StopCompanion => "stop-companion",
            ManagementIntentKind::DeleteCompanion => "delete-companion",
            ManagementIntentKind::CancelTask => "cancel-task",
            ManagementIntentKind::ResumeTask => "resume-task",
            ManagementIntentKind::SelectWorkspace => "select-workspace",
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

    async fn cancel_task_intent(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        if let Some(answer) = self
            .replay_or_hold(
                frame,
                live,
                intent,
                Self::intent_fingerprint(intent, Self::INTENT_KIND_CANCEL_TASK),
            )
            .await
        {
            return answer;
        }
        let Some(task) = parse_task_target(&intent.target)
            .map(RawId::from_uuid)
            .map(TaskId::from_raw)
        else {
            return vec![outcome_frame(
                frame,
                live,
                intent,
                self.record_decided(
                    Self::intent_fingerprint(intent, Self::INTENT_KIND_CANCEL_TASK),
                    IntentOutcome::NeedsClarification,
                )
                .await,
            )];
        };
        match self.cancel_task(CancelTaskCommand { task }).await {
            Ok(TaskCancelOutcome::CancelAccepted | TaskCancelOutcome::AlreadyCancelled) => {
                vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    self.record_decided(
                        Self::intent_fingerprint(intent, Self::INTENT_KIND_CANCEL_TASK),
                        IntentOutcome::AppliedAsOneTime,
                    )
                    .await,
                )]
            }
            Ok(
                TaskCancelOutcome::TaskTerminal { .. }
                | TaskCancelOutcome::MissingTask { .. }
                | TaskCancelOutcome::Superseded,
            ) => {
                vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    self.record_decided(
                        Self::intent_fingerprint(intent, Self::INTENT_KIND_CANCEL_TASK),
                        IntentOutcome::NeedsClarification,
                    )
                    .await,
                )]
            }
            Err(_) => vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::HeldByOperation,
            )],
        }
    }

    async fn resume_task_intent(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        if let Some(answer) = self
            .replay_or_hold(
                frame,
                live,
                intent,
                Self::intent_fingerprint(intent, Self::INTENT_KIND_RESUME_TASK),
            )
            .await
        {
            return answer;
        }
        let Some(task) = parse_task_target(&intent.target)
            .map(RawId::from_uuid)
            .map(TaskId::from_raw)
        else {
            return vec![outcome_frame(
                frame,
                live,
                intent,
                self.record_decided(
                    Self::intent_fingerprint(intent, Self::INTENT_KIND_RESUME_TASK),
                    IntentOutcome::NeedsClarification,
                )
                .await,
            )];
        };
        let Some(body) = intent
            .rationale
            .quote
            .clone()
            .filter(|quote| !quote.trim().is_empty())
        else {
            return vec![outcome_frame(
                frame,
                live,
                intent,
                self.record_decided(
                    Self::intent_fingerprint(intent, Self::INTENT_KIND_RESUME_TASK),
                    IntentOutcome::NeedsClarification,
                )
                .await,
            )];
        };
        let record = match self.store.load_task(task).await {
            Err(_) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::HeldByOperation,
                )];
            }
            Ok(None) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    self.record_decided(
                        Self::intent_fingerprint(intent, Self::INTENT_KIND_RESUME_TASK),
                        IntentOutcome::NeedsClarification,
                    )
                    .await,
                )];
            }
            Ok(Some(record)) => record,
        };
        let activity = match self
            .store
            .record_resume_activity(RecordResumeActivityCommand {
                companion: CompanionId::from_raw(record.task.assignee.companion),
                task: record.task.reference,
                purpose: record.task.purpose,
                body,
                command: RawId::from_uuid(intent.intent_id.0),
            })
            .await
        {
            Err(_) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::HeldByOperation,
                )];
            }
            Ok(ResumeActivityOutcome::HeldForErasure) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::HeldByOperation,
                )];
            }
            Ok(ResumeActivityOutcome::Recorded(activity)) => activity,
        };
        match self
            .resume_task(ResumeTaskCommand {
                premise: SteeringPremiseRef {
                    expected: record.task.reference,
                    purpose: record.task.purpose,
                },
                instruction: ResumeInstructionSource::OwnerManagement {
                    activity: activity.as_raw(),
                },
            })
            .await
        {
            Ok(TaskResumeOutcome::Resumed { .. }) => vec![outcome_frame(
                frame,
                live,
                intent,
                self.record_decided(
                    Self::intent_fingerprint(intent, Self::INTENT_KIND_RESUME_TASK),
                    IntentOutcome::AppliedAsOneTime,
                )
                .await,
            )],
            Ok(_) => vec![outcome_frame(
                frame,
                live,
                intent,
                self.record_decided(
                    Self::intent_fingerprint(intent, Self::INTENT_KIND_RESUME_TASK),
                    IntentOutcome::NeedsClarification,
                )
                .await,
            )],
            Err(_) => vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::HeldByOperation,
            )],
        }
    }

    async fn select_workspace_intent(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        if let Some(answer) = self
            .replay_or_hold(
                frame,
                live,
                intent,
                Self::intent_fingerprint(intent, Self::INTENT_KIND_SELECT_WORKSPACE),
            )
            .await
        {
            return answer;
        }
        let validated = parse_workspace_target(&intent.target)
            .and_then(|path| WorkspaceRoot::open(path).ok())
            .map(|root| WorkspaceFolderRef {
                path: root.as_path().to_string_lossy().into_owned(),
            });
        let Some(folder) = validated else {
            return vec![outcome_frame(
                frame,
                live,
                intent,
                self.record_decided(
                    Self::intent_fingerprint(intent, Self::INTENT_KIND_SELECT_WORKSPACE),
                    IntentOutcome::NeedsClarification,
                )
                .await,
            )];
        };
        self.trusted_task_premises.set_workspace(folder);
        vec![outcome_frame(
            frame,
            live,
            intent,
            self.record_decided(
                Self::intent_fingerprint(intent, Self::INTENT_KIND_SELECT_WORKSPACE),
                IntentOutcome::AppliedAsOneTime,
            )
            .await,
        )]
    }

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
                    Self::intent_fingerprint(intent, Self::INTENT_KIND_REGISTER),
                    IntentOutcome::NeedsClarification,
                )
                .await,
            )];
        };
        if let Some(answer) = self
            .replay_or_hold(
                frame,
                live,
                intent,
                Self::intent_fingerprint(intent, Self::INTENT_KIND_REGISTER),
            )
            .await
        {
            return answer;
        }
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

    const INTENT_KIND_ASSIGN: &str = "assign";
    const INTENT_KIND_REGISTER: &str = "register";
    const INTENT_KIND_COMPLETE: &str = "complete";
    const INTENT_KIND_CANCEL_TASK: &str = "cancel-task";
    const INTENT_KIND_RESUME_TASK: &str = "resume-task";
    const INTENT_KIND_SELECT_WORKSPACE: &str = "select-workspace";

    pub(crate) fn rationale_origin_name(origin: RationaleOrigin) -> &'static str {
        match origin {
            RationaleOrigin::Conversation => "conversation",
            RationaleOrigin::ManagementSurface => "management-surface",
        }
    }

    pub(crate) fn intent_fingerprint(intent: &ManagementIntent, kind: &str) -> IntentFingerprint {
        IntentFingerprint {
            intent_id: intent.intent_id.0.as_hyphenated().to_string(),
            kind: kind.to_string(),
            target: intent.target.0.clone(),
            base: intent.base_view.0.clone(),
            rationale_origin: Self::rationale_origin_name(intent.rationale.origin).to_string(),
            rationale_quote: intent.rationale.quote.clone(),
        }
    }

    fn fingerprint_matches(stored: &IntentFingerprint, incoming: &IntentFingerprint) -> bool {
        stored.kind == incoming.kind
            && stored.target == incoming.target
            && stored.base == incoming.base
            && stored.rationale_origin == incoming.rationale_origin
            && stored.rationale_quote == incoming.rationale_quote
    }

    fn replayed_outcome(snapshot: &IntentOutcome) -> ManagementOutcome {
        match snapshot {
            IntentOutcome::StoredAsRuleView { revision } => ManagementOutcome::StoredAsRuleView {
                revision: ViewMarkWire(revision.clone()),
            },
            IntentOutcome::AppliedAsOneTime => ManagementOutcome::AppliedAsOneTime,
            IntentOutcome::HeldByOperation => ManagementOutcome::HeldByOperation,
            IntentOutcome::NeedsClarification | IntentOutcome::RevisionExhausted => {
                ManagementOutcome::NeedsClarification
            }
            IntentOutcome::StaleBaseView { current } => ManagementOutcome::StaleBaseView {
                current: ViewMarkWire(current.clone()),
            },
        }
    }

    pub(crate) async fn replay_or_hold(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        intent: &ManagementIntent,
        fingerprint: IntentFingerprint,
    ) -> Option<Vec<WireFrame>> {
        let intent_key = fingerprint.intent_id.clone();
        match self.store.lookup_intent_outcome(&intent_key).await {
            Err(_) => Some(vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::HeldByOperation,
            )]),
            Ok(Some(stored)) if Self::fingerprint_matches(&stored.fingerprint, &fingerprint) => {
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
            let view = self.build_view(&[], None, None, None, live).await;
            return vec![view_frame(frame, live, view)];
        }
        if target == SETUP_COMPLETE_TARGET {
            return self.complete_setup(frame, intent, live).await;
        }
        if crate::usage::is_usage_cap_target(target) {
            return self.set_usage_cap_intent(frame, intent, live).await;
        }
        let Some((capability, provider, model, credential_id)) =
            parse_consent_target(&intent.target)
        else {
            return vec![outcome_frame(
                frame,
                live,
                intent,
                self.record_decided(
                    Self::intent_fingerprint(intent, Self::INTENT_KIND_ASSIGN),
                    IntentOutcome::NeedsClarification,
                )
                .await,
            )];
        };
        let Some(capability) = CapabilityKind::from_name(&capability) else {
            return vec![outcome_frame(
                frame,
                live,
                intent,
                self.record_decided(
                    Self::intent_fingerprint(intent, Self::INTENT_KIND_ASSIGN),
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

    pub(crate) async fn record_decided(
        &self,
        fingerprint: IntentFingerprint,
        outcome: IntentOutcome,
    ) -> ManagementOutcome {
        let answer = Self::replayed_outcome(&outcome);
        let record = IntentOutcomeRecord {
            fingerprint,
            outcome,
        };
        match self.store.record_intent_outcome(record).await {
            Ok(IntentResolution::Decided(())) => answer,
            Ok(IntentResolution::Replay(stored)) => Self::replayed_outcome(&stored.outcome),
            Ok(IntentResolution::Conflict(_)) => ManagementOutcome::NeedsClarification,
            Err(_) => ManagementOutcome::HeldByOperation,
        }
    }

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
        if let Some(answer) = self
            .replay_or_hold(
                frame,
                live,
                intent,
                Self::intent_fingerprint(intent, Self::INTENT_KIND_ASSIGN),
            )
            .await
        {
            return answer;
        }
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
        if let Some(answer) = self
            .replay_or_hold(
                frame,
                live,
                intent,
                Self::intent_fingerprint(intent, Self::INTENT_KIND_COMPLETE),
            )
            .await
        {
            return answer;
        }
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
                live,
            )
            .await;
        vec![view_frame(frame, live, view)]
    }

    pub(crate) async fn build_view(
        &self,
        wanted: &[String],
        memory_after: Option<&str>,
        memory_revisions_of: Option<&str>,
        memory_revisions_after: Option<u64>,
        live: &LiveInput,
    ) -> ManagementView {
        let wants = |name: &str| wanted.is_empty() || wanted.iter().any(|section| section == name);
        let mut sections = Vec::new();
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
                CredStore::Os(_) => "os-protected-store",
                CredStore::MemoryVersioned(_) => "memory-versioned",
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
            let coverage = self.current_coverage().await;
            let (mut body, delivered) = self
                .render_memory_view(
                    memory_after,
                    memory_revisions_of,
                    memory_revisions_after,
                    &coverage,
                )
                .await;
            if delivered && !self.note_client_body_delivery(live).await {
                body.clear();
            } else if delivered {
                let fresh = self.current_coverage().await;
                if fresh.covers(&body) {
                    body.clear();
                }
            }
            sections.push(ViewSection {
                kind: String::from("memory"),
                title: String::from("Memory"),
                body,
            });
        }
        ManagementView {
            mark: ViewMarkWire(mark),
            sections,
        }
    }

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

    async fn render_memory_view(
        &self,
        after: Option<&str>,
        revisions_of: Option<&str>,
        after_revision: Option<u64>,
        coverage: &CurrentCoverage,
    ) -> (String, bool) {
        match revisions_of {
            Some(raw) => {
                self.render_revision_page(raw, after_revision, coverage)
                    .await
            }
            None => self.render_memory_list(after, coverage).await,
        }
    }

    async fn render_memory_list(
        &self,
        after: Option<&str>,
        coverage: &CurrentCoverage,
    ) -> (String, bool) {
        let Ok(companion) = self.store.ensure_running_companion().await else {
            return (String::from("unavailable"), false);
        };
        let cursor = match after {
            Some(raw) => match parse_memory_id(raw) {
                Some(id) => Some(id),
                None => return (String::from("invalid cursor"), false),
            },
            None => None,
        };
        let Ok(memories) = self
            .store
            .list_current_memories(companion.as_raw(), cursor, MEMORY_PAGE_SIZE + 1)
            .await
        else {
            return (String::from("unavailable"), false);
        };
        if memories.is_empty() {
            return if after.is_some() {
                (String::from("no older memories"), false)
            } else {
                (String::from("(none)"), false)
            };
        }
        let mut body = String::new();
        let mut delivered = false;
        let mut last_id = memories[0].id;
        let mut rendered = 0_usize;
        for memory in memories.iter().take(MEMORY_PAGE_SIZE as usize) {
            let (line, line_delivered) = render_memory(memory, coverage);
            if rendered > 0 && body.len() + line.len() > MEMORY_BODY_BUDGET {
                break;
            }
            body.push_str(&line);
            body.push('\n');
            delivered |= line_delivered;
            last_id = memory.id;
            rendered += 1;
        }
        if rendered < memories.len() {
            body.push_str(&format!("next: {}\n", last_id.as_raw().as_uuid()));
        }
        (body.trim_end().to_owned(), delivered)
    }

    async fn render_revision_page(
        &self,
        memory: &str,
        after_revision: Option<u64>,
        coverage: &CurrentCoverage,
    ) -> (String, bool) {
        let Some(memory_id) = parse_memory_id(memory) else {
            return (String::from("invalid cursor"), false);
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
            return (String::from("unavailable"), false);
        };
        if revisions.is_empty() {
            return if after.is_some() {
                (String::from("no more revisions"), false)
            } else {
                (String::from("unknown memory"), false)
            };
        }
        let summary_ids: Vec<SummaryId> = revisions.iter().filter_map(|r| r.summary).collect();
        let loaded = self.store.load_summaries(&summary_ids).await;
        let summaries_unavailable = loaded.is_err();
        let loaded = loaded.unwrap_or_default();
        let mut body = String::new();
        let mut delivered = false;
        let mut last_revision = revisions[0].revision;
        let mut rendered = 0_usize;
        for revision in revisions.iter().take(MEMORY_REVISION_PAGE_SIZE as usize) {
            let (mut piece, revision_delivered) = render_revision(revision, coverage);
            let mut piece_delivered = revision_delivered;
            if let Some(summary_id) = revision.summary {
                let short = short_id(summary_id.as_raw());
                match loaded.iter().find(|summary| summary.id == summary_id) {
                    Some(summary) => {
                        let covered = coverage.covers(&summary.content);
                        piece_delivered |= !covered && !summary.content.is_empty();
                        let grounds = if covered {
                            ""
                        } else {
                            summary.content.as_str()
                        };
                        piece.push_str(&format!("  grounds summary {short}: {grounds}\n"));
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
            delivered |= piece_delivered;
            last_revision = revision.revision;
            rendered += 1;
        }
        if rendered < revisions.len() {
            body.push_str(&format!("next-revision: {}\n", last_revision.as_u64()));
        }
        (body.trim_end().to_owned(), delivered)
    }
}

fn presence_text(present: bool, source: &str) -> String {
    if present {
        format!("present ({source})")
    } else {
        String::from("absent")
    }
}

const MEMORY_PAGE_SIZE: u64 = 20;

const MEMORY_REVISION_PAGE_SIZE: u64 = 20;

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

fn render_memory(memory: &Memory, coverage: &CurrentCoverage) -> (String, bool) {
    let covered = coverage.covers(&memory.content);
    // One field per line: an embedded newline in the body would split the
    // parser's line-oriented framing, so it is flattened to spaces.
    let content = if covered {
        String::new()
    } else {
        memory.content.replace(['\r', '\n'], " ")
    };
    (
        format!(
            "memory {} scope=companion importance={} temporal={} recall={} revision={} updated={}\ncontent: {content}\n",
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
        ),
        !covered && !memory.content.is_empty(),
    )
}

fn render_revision(revision: &MemoryRevisionRecord, coverage: &CurrentCoverage) -> (String, bool) {
    let covered = coverage.covers(&revision.content);
    // Same one-field-per-line framing as [`render_memory`]: a newline in the
    // body must not become a spurious `grounds` continuation line.
    let content = if covered {
        String::new()
    } else {
        revision.content.replace(['\r', '\n'], " ")
    };
    (
        format!(
            "  rev{} {} at={} content: {content}\n",
            revision.revision.as_u64(),
            change_label(revision.change),
            revision.at.to_rfc3339(),
        ),
        !covered && !revision.content.is_empty(),
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
