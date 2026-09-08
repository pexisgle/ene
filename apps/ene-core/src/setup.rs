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
//!   knows it and the store holds its bearer), then saves consent at revision
//!   previous-plus-one and answers
//!   [`StoredAsRuleView`](ene_api::v1::management::ManagementOutcome::StoredAsRuleView)
//!   carrying the new revision mark.
//! - `(ManageRuleConsentCap, "setup:complete")` verifies consent plus
//!   credential presence and answers `AppliedAsOneTime`. Setup completion is
//!   derived thereafter (consent stored and credential present), never written
//!   as a flag.
//! - `(ManageRuleConsentCap, "setup:show")` answers a [`ManagementView`]
//!   instead of an outcome.
//!
//! The `NeedsClarification` DTO carries no detail string, so the "not in
//! `Stage 2` scope" note lives here in documentation, not on the wire.
//! `base_view` staleness is checked on the consent-writing paths against the
//! current consent mark; mismatch answers
//! [`StaleBaseView`](ene_api::v1::management::ManagementOutcome::StaleBaseView).
//! Registration takes no mark (no revision is involved) and views are reads.
//!
//! Views never carry secrets: sections report provider, model, consent
//! revision, and credential presence only. A store failure behind a view
//! answers zero sections under the `"unavailable"` mark (documented gap: there
//! is no error DTO on the view path).

use ene_api::v1::management::{
    ManagementIntent, ManagementIntentKind, ManagementOutcome, ManagementView,
    ManagementViewRequest, ViewSection,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::ViewMarkWire;
use ene_credential::{
    CredentialRef, CredentialRefRepository, CredentialStore, RegisterCredentialCommand,
    RegisterOutcome, register,
};
use ene_permission::{ConsentRecord, ConsentRepository, ConsentRevision};
use ene_plugin_ipc::WireFrame;
use ene_primitive::RawId;

use crate::serve::{CredStore, HostHandle, outgoing_envelope, outgoing_frame};

/// Builds a management outcome reply, echoing the intent id.
///
/// The outcome is the ack of the intent saga, so the envelope carries the
/// intent id as `command_id` alongside the `reply_to` link.
fn outcome_frame(
    frame: &WireFrame,
    intent: &ManagementIntent,
    outcome: ManagementOutcome,
) -> WireFrame {
    let mut envelope = outgoing_envelope("ManagementOutcome", Some(frame.envelope.message_id));
    envelope.correlation.command_id = Some(intent.intent_id);
    WireFrame {
        envelope,
        payload: WirePayload::ManagementOutcome(outcome),
    }
}

/// Builds a management view reply.
fn view_frame(frame: &WireFrame, view: ManagementView) -> WireFrame {
    outgoing_frame(frame, "ManagementView", WirePayload::ManagementView(view))
}

/// Splits a `"credential:{provider}:{label}"` target.
fn split_credential_target(target: &str) -> Option<(String, String)> {
    let rest = target.strip_prefix("credential:")?;
    let (provider, label) = rest.split_once(':')?;
    if provider.is_empty() || label.is_empty() {
        return None;
    }
    Some((provider.to_string(), label.to_string()))
}

/// Splits a `"consent:{provider}:{model}:{credential-id}"` target.
///
/// The credential id keeps its remainder verbatim (it is itself a
/// `provider:label` pair), so the split is capped at three parts.
fn split_consent_target(target: &str) -> Option<(String, String, String)> {
    let rest = target.strip_prefix("consent:")?;
    let mut parts = rest.splitn(3, ':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(provider), Some(model), Some(credential))
            if !provider.is_empty() && !model.is_empty() && !credential.is_empty() =>
        {
            Some((
                provider.to_string(),
                model.to_string(),
                credential.to_string(),
            ))
        }
        _ => None,
    }
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

    /// Reads the current consent display mark, if the store answers.
    ///
    /// The mark is `"consent-rev-{n}"` over the stored revision, or
    /// `"consent-none"` when nothing is stored. [`None`] means the store
    /// failed and the caller must hold rather than decide.
    pub(crate) async fn current_mark(&self) -> Option<String> {
        let Ok(current) = self.store.load_current().await else {
            return None;
        };
        match current {
            Some(record) => Some(format!("consent-rev-{}", record.rev.as_u64())),
            None => Some(String::from("consent-none")),
        }
    }

    /// Reports whether setup is complete: consent stored and bearer present.
    ///
    /// Returns [`None`] when the store fails, so callers hold rather than
    /// treat an unreadable store as incomplete setup.
    pub(crate) async fn setup_ready(&self) -> Option<bool> {
        let Ok(current) = self.store.load_current().await else {
            return None;
        };
        let Some(consent) = current else {
            return Some(false);
        };
        let Ok(refs) = self.store.list_refs().await else {
            return None;
        };
        let credential = match refs.iter().find(|known| known.id == consent.credential_id) {
            Some(known) => known.clone(),
            None => return Some(false),
        };
        Some(self.cred_store.contains(&credential))
    }

    /// Maps one [`ManagementIntent`] to its `Stage 2` outcome frames.
    ///
    /// Setup pairs route through the register/assign/complete/show paths
    /// below; every other kind answers `NeedsClarification` (deferred scope,
    /// never a silent accept). Store failures behind a write answer
    /// [`HeldByOperation`](ene_api::v1::management::ManagementOutcome::HeldByOperation):
    /// nothing was decided, so a later retry is safe.
    pub(crate) async fn apply_intent(
        &mut self,
        frame: &WireFrame,
        intent: &ManagementIntent,
    ) -> Vec<WireFrame> {
        match intent.kind {
            ManagementIntentKind::ConfigureCredentialIntent => {
                self.register_credential(frame, intent).await
            }
            ManagementIntentKind::ManageRuleConsentCap => {
                self.apply_consent_or_setup(frame, intent).await
            }
            _ => vec![outcome_frame(
                frame,
                intent,
                ManagementOutcome::NeedsClarification,
            )],
        }
    }

    /// Registers the credential ref named by a `credential:` target.
    async fn register_credential(
        &mut self,
        frame: &WireFrame,
        intent: &ManagementIntent,
    ) -> Vec<WireFrame> {
        let Some((provider, label)) = split_credential_target(&intent.target.0) else {
            return vec![outcome_frame(
                frame,
                intent,
                ManagementOutcome::NeedsClarification,
            )];
        };
        let outcome =
            match register(RegisterCredentialCommand { provider, label }, &self.store).await {
                Ok(RegisterOutcome::Registered(_) | RegisterOutcome::AlreadyExists(_)) => {
                    ManagementOutcome::AppliedAsOneTime
                }
                Ok(RegisterOutcome::InvalidProvider) => ManagementOutcome::NeedsClarification,
                Err(_) => ManagementOutcome::HeldByOperation,
            };
        vec![outcome_frame(frame, intent, outcome)]
    }

    /// Routes a consent-scope intent to assign, complete, or show.
    async fn apply_consent_or_setup(
        &mut self,
        frame: &WireFrame,
        intent: &ManagementIntent,
    ) -> Vec<WireFrame> {
        let target = intent.target.0.as_str();
        if target == "setup:show" {
            let view = self.build_view(&[]).await;
            return vec![view_frame(frame, view)];
        }
        if target == "setup:complete" {
            return self.complete_setup(frame, intent).await;
        }
        let Some((provider, model, credential_id)) = split_consent_target(target) else {
            return vec![outcome_frame(
                frame,
                intent,
                ManagementOutcome::NeedsClarification,
            )];
        };
        self.assign_consent(frame, intent, &provider, &model, &credential_id)
            .await
    }

    /// Assigns the consent route after verifying credential presence.
    ///
    /// Checks the intent `base_view` against the current consent mark first
    /// (stale views re-read and retry), then requires the credential to be
    /// both registered and bearer-present. The saved record keeps the stored
    /// id when one exists and bumps its revision by one, saturating.
    async fn assign_consent(
        &mut self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        provider: &str,
        model: &str,
        credential_id: &str,
    ) -> Vec<WireFrame> {
        let Some(mark) = self.current_mark().await else {
            return vec![outcome_frame(
                frame,
                intent,
                ManagementOutcome::HeldByOperation,
            )];
        };
        if intent.base_view.0 != mark {
            return vec![outcome_frame(
                frame,
                intent,
                ManagementOutcome::StaleBaseView {
                    current: ViewMarkWire(mark),
                },
            )];
        }
        let Ok(refs) = self.store.list_refs().await else {
            return vec![outcome_frame(
                frame,
                intent,
                ManagementOutcome::HeldByOperation,
            )];
        };
        let Some(credential) = refs.iter().find(|known| known.id == credential_id).cloned() else {
            return vec![outcome_frame(
                frame,
                intent,
                ManagementOutcome::NeedsClarification,
            )];
        };
        if !self.cred_store.contains(&credential) {
            return vec![outcome_frame(
                frame,
                intent,
                ManagementOutcome::NeedsClarification,
            )];
        }
        let Ok(current) = self.store.load_current().await else {
            return vec![outcome_frame(
                frame,
                intent,
                ManagementOutcome::HeldByOperation,
            )];
        };
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
        match self.store.save_current(record).await {
            Ok(()) => vec![outcome_frame(
                frame,
                intent,
                ManagementOutcome::StoredAsRuleView {
                    revision: ViewMarkWire(next_rev.to_string()),
                },
            )],
            Err(_) => vec![outcome_frame(
                frame,
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
        &mut self,
        frame: &WireFrame,
        intent: &ManagementIntent,
    ) -> Vec<WireFrame> {
        let Some(mark) = self.current_mark().await else {
            return vec![outcome_frame(
                frame,
                intent,
                ManagementOutcome::HeldByOperation,
            )];
        };
        if intent.base_view.0 != mark {
            return vec![outcome_frame(
                frame,
                intent,
                ManagementOutcome::StaleBaseView {
                    current: ViewMarkWire(mark),
                },
            )];
        }
        match self.setup_ready().await {
            Some(true) => vec![outcome_frame(
                frame,
                intent,
                ManagementOutcome::AppliedAsOneTime,
            )],
            Some(false) => vec![outcome_frame(
                frame,
                intent,
                ManagementOutcome::NeedsClarification,
            )],
            None => vec![outcome_frame(
                frame,
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
        &mut self,
        frame: &WireFrame,
        request: &ManagementViewRequest,
    ) -> Vec<WireFrame> {
        let view = self.build_view(&request.sections).await;
        vec![view_frame(frame, view)]
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
        let (provider_text, model_text, consent_text, mark_text) = match &current {
            Some(record) => (
                record.provider.clone(),
                record.model.clone(),
                format!("rev {}", record.rev.as_u64()),
                format!("consent-rev-{}", record.rev.as_u64()),
            ),
            None => (
                String::from("unconfigured"),
                String::from("unconfigured"),
                String::from("none"),
                String::from("consent-none"),
            ),
        };
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
