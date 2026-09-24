use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ene_api::v1::management::{ManagementOutcome, ManagementViewRequest};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::StreamWireId;
use ene_api::v1::round::{HistoryItem, PresentationStatus};
use ene_body::ipc::{
    AssetRef, LocalUiFact, MotionSetInfo, ParentToBody, PlacementBox, PoseClip, PoseHint,
    PresentationFeedback,
};
use ene_client::{Client, PendingPairingClient};
use ene_local_control::{ControlOp, ControlOutcome, FromConfirmation};

use crate::body_supervise::{BodyStatus, BodySupervisor};
use crate::control::ConfirmationClient;
use crate::erasure::{self, GuiOwned};
use crate::host_launch;
use crate::i18n::Locale;
use crate::measure::WaylandFeedbackTraceLine;
use crate::motion::{self, MotionEnvironment};
use crate::secret::SecretIntake;
use crate::session::{
    self, ChatDeliveryPhase, ChatOperation, ChatSendReport, ChatSessionOutcome,
    ChatTechnicalFailure, ChatTurn, SETUP_PROVIDER_OPENAI, SetupFacts,
};
use crate::ui::deletion::DeletionPanel;
use crate::ui::tasks::TaskPanel;
use crate::ui::usage::UsagePanel;
use crate::ui::{
    Composer, DEFAULT_REQUEST_TIMEOUT, DesktopError, GuiSnapshot, MemoryPage, Page, WizardStep,
    history_lines, request_with_timeout,
};
use crate::{BUNDLED_SAMPLE_ASSET, DESKTOP_DESCRIPTOR};

const LOCALE_FILE: &str = "desktop-locale";
const DEFAULT_MODEL: &str = "gpt-5.6-luna";

pub struct DesktopRuntime {
    data_dir: PathBuf,
    locale: Locale,
    page: Page,
    wizard_step: WizardStep,
    composer: Composer,
    secret: SecretIntake,
    client: Option<Client>,
    pending_pairing: Option<PendingPairingClient>,
    control: Option<ConfirmationClient>,
    timeline: Vec<super::presentation::Message>,
    surface_erasure: Option<super::presentation::SurfaceErasure>,
    history: Vec<HistoryItem>,
    facts: SetupFacts,
    body: BodySupervisor,
    body_status: BodyStatus,
    body_placement: PlacementBox,
    body_hidden: bool,
    body_pose_deadline: Option<Instant>,
    presentation_trace: Option<std::fs::File>,
    model: String,
    memory: MemoryPage,
    tasks: TaskPanel,
    usage: UsagePanel,
    deletion: DeletionPanel,
    chat_receipt: Option<(String, Option<StreamWireId>)>,
}

impl DesktopRuntime {
    #[must_use]
    pub fn new(data_dir: PathBuf) -> Self {
        let locale = load_locale(&data_dir);
        Self {
            data_dir,
            locale,
            page: Page::Wizard,
            wizard_step: WizardStep::Language,
            composer: Composer::default(),
            secret: SecretIntake::new(),
            client: None,
            pending_pairing: None,
            control: None,
            timeline: Vec::new(),
            surface_erasure: None,
            history: Vec::new(),
            facts: SetupFacts {
                credential_present: false,
                consent_assigned: false,
                model: None,
                mark: String::new(),
            },
            body: BodySupervisor::new(),
            body_status: BodyStatus::Absent,
            body_placement: PlacementBox {
                x: 24,
                y: 24,
                width: 420,
                height: 640,
                scale: 1.0,
            },
            body_hidden: false,
            body_pose_deadline: None,
            presentation_trace: std::env::var_os("ENE_PRESENTATION_TRACE_JSONL").and_then(|path| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .ok()
            }),
            model: String::from(DEFAULT_MODEL),
            memory: MemoryPage::default(),
            tasks: TaskPanel::default(),
            usage: UsagePanel::default(),
            deletion: DeletionPanel::default(),
            chat_receipt: None,
        }
    }

    pub fn tick(&mut self) {
        self.body_status = self.body.poll();
        while let Some(fact) = self.body.take_local_ui() {
            match fact {
                LocalUiFact::Drag { x, y } => {
                    self.body_placement.x = x;
                    self.body_placement.y = y;
                }
                LocalUiFact::Resize { width, height } => {
                    self.body_placement.width = width;
                    self.body_placement.height = height;
                }
                LocalUiFact::Hide => self.body_hidden = true,
            }
        }
        while let Some(feedback) = self.body.take_presentation() {
            self.append_presentation_trace(&feedback);
        }
        if self
            .body_pose_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.body_pose_deadline = None;
            self.project_body_pose(PoseHint::Idle);
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> GuiSnapshot {
        GuiSnapshot {
            locale: self.locale.as_tag().to_string(),
            page: format!("{:?}", self.page),
            timeline: self
                .timeline
                .iter()
                .map(|m| {
                    format!(
                        "[{}] {}",
                        if m.owner { "owner" } else { "companion" },
                        m.text
                    )
                })
                .collect(),
            history: history_lines(&self.history),
            draft: self.composer.draft().to_string(),
            composing: self.composer.composing(),
            tasks: self.tasks.list_lines(),
            task_detail: self.tasks.detail_text(),
            body_status: format!("{:?}", self.body_status),
            setup_ready: self.facts.setup_ready(),
            credential_present: self.facts.credential_present,
            consent_assigned: self.facts.consent_assigned,
            secret_visible: matches!(self.wizard_step, WizardStep::Credential),
            memories: self.memory.rows().to_vec(),
            memory_revisions: self.memory.revisions().to_vec(),
            memory_panel: self.memory.panel(),
            usage_body: self.usage.render(),
            deletion_body: self.deletion.render(),
        }
    }

    pub fn set_locale(&mut self, locale: Locale) {
        self.locale = locale;
        persist_locale(&self.data_dir, locale);
    }

    pub fn open_page(&mut self, page: Page) {
        self.page = page;
    }

    pub fn wizard_next(&mut self) {
        if let Some(next) = self.wizard_step.next() {
            self.wizard_step = next;
        }
    }

    pub fn wizard_back(&mut self) {
        if let Some(back) = self.wizard_step.back() {
            self.wizard_step = back;
        }
    }

    pub fn composer_mut(&mut self) -> &mut Composer {
        &mut self.composer
    }

    pub fn set_secret(&mut self, value: String) {
        self.secret.set(value);
    }

    pub fn cancel_secret_keep_pending(&mut self) {
        self.secret.cancel();
        if matches!(self.page, Page::Confirm) {
            self.page = Page::Wizard;
        }
    }

    pub fn cancel_secret(&mut self) {
        self.cancel_secret_keep_pending();
        if let Some(seat) = &mut self.control {
            seat.discard_pending();
        }
    }

    pub fn set_model(&mut self, model: String) {
        self.model = model;
    }

    pub fn try_spawn_body(&mut self, exe: &Path) {
        let asset = self.bundled_sample_asset();
        if !asset.is_file() {
            self.body.shutdown();
            self.body_status = BodyStatus::Absent;
            return;
        }
        self.body_status = self.body.spawn_if_present(exe);
        if self.body_status != BodyStatus::Spawned {
            return;
        }
        let mut commands = vec![ParentToBody::AssetRef(AssetRef::Path {
            path: asset.to_string_lossy().into_owned(),
        })];
        let clips = self.motion_plan();
        if !clips.is_empty() {
            commands.push(ParentToBody::MotionSet(MotionSetInfo { clips }));
        }
        commands.push(ParentToBody::Placement(self.body_placement));
        commands.push(ParentToBody::PoseHint(PoseHint::Idle));
        commands.push(ParentToBody::Show);
        for command in commands {
            if self.body.send_projection(&command).is_err() {
                self.body_status = BodyStatus::Exited;
                break;
            }
        }
        self.body_hidden = self.body_status != BodyStatus::Spawned;
    }

    #[must_use]
    pub fn motion_plan(&self) -> Vec<PoseClip> {
        motion::resolve(&MotionEnvironment::from_process(&self.data_dir))
    }

    pub fn try_spawn_located_body(&mut self) {
        if let Some(executable) = BodySupervisor::locate_binary() {
            self.try_spawn_body(&executable);
        } else {
            self.body.shutdown();
            self.body_status = BodyStatus::Absent;
        }
    }

    pub fn project_body_pose(&mut self, pose: PoseHint) {
        self.body_pose_deadline = matches!(pose, PoseHint::Speaking | PoseHint::Attention)
            .then(|| Instant::now() + Duration::from_secs(2));
        if self.body_status == BodyStatus::Spawned {
            let _result = self.body.send_projection(&ParentToBody::PoseHint(pose));
        }
    }

    pub fn show_body(&mut self) {
        if self.body_status == BodyStatus::Spawned
            && self.body.available()
            && self.body.send_projection(&ParentToBody::Show).is_ok()
        {
            self.body_hidden = false;
        }
    }

    pub fn hide_body(&mut self) {
        if self.body_status == BodyStatus::Spawned
            && self.body.available()
            && self.body.send_projection(&ParentToBody::Hide).is_ok()
        {
            self.body_hidden = true;
        }
    }

    fn append_presentation_trace(&mut self, feedback: &PresentationFeedback) {
        let Some(file) = &mut self.presentation_trace else {
            return;
        };
        let Ok(since_epoch) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        else {
            return;
        };
        let Ok(observed_unix_ns) = u64::try_from(since_epoch.as_nanos()) else {
            return;
        };
        let line = WaylandFeedbackTraceLine {
            observed_unix_ns,
            feedback: feedback.clone(),
        };
        let Ok(mut encoded) = serde_json::to_vec(&line) else {
            return;
        };
        encoded.push(b'\n');
        let _result = file.write_all(&encoded);
    }

    pub fn kill_body(&mut self) {
        self.body.shutdown();
        self.body_status = self.body.poll();
    }

    #[must_use]
    pub fn body_motion_ready(&mut self) -> bool {
        self.body.motion_ready()
    }

    pub fn bundled_sample_asset(&self) -> PathBuf {
        if let Some(path) = std::env::var_os("ENE_BUNDLED_ASSET_PATH")
            .map(PathBuf::from)
            .filter(|path| path.is_file())
        {
            return path;
        }
        let env = MotionEnvironment::from_process(&self.data_dir);
        let data_candidate = env.data_dir.join(BUNDLED_SAMPLE_ASSET);
        for root in motion::install_roots(&env) {
            let candidate = root.join(BUNDLED_SAMPLE_ASSET);
            if candidate.is_file() {
                return candidate;
            }
        }
        data_candidate
    }

    pub fn ensure_host(&mut self) -> Result<(), DesktopError> {
        host_launch::ensure_serving(&self.data_dir)
            .map_err(|error| DesktopError::HostLaunch(error.to_string()))
    }

    pub fn attach_confirmation(
        &mut self,
        channel: ene_local_control::GuiChannel,
    ) -> Result<(), DesktopError> {
        self.control = Some(ConfirmationClient::adopt(&self.data_dir, channel)?);
        Ok(())
    }

    pub async fn connect_or_begin_pairing(&mut self) -> Result<(), DesktopError> {
        self.require_confirmation()?;
        match session::connect_or_pending(&self.data_dir, DESKTOP_DESCRIPTOR).await? {
            session::DesktopConnect::Paired(client) => {
                self.adopt_client(*client);
                self.refresh_after_connect().await;
                Ok(())
            }
            session::DesktopConnect::PendingOwnerConfirmation(pending) => {
                let pending_id = pending.pending_id().to_owned();
                let seat = self.require_confirmation_mut()?;
                seat.request_device_approve(&pending_id).await?;
                self.pending_pairing = Some(pending);
                self.page = Page::Confirm;
                Ok(())
            }
        }
    }

    fn require_confirmation(&self) -> Result<(), DesktopError> {
        match self.control {
            Some(_) => Ok(()),
            None => Err(DesktopError::Protocol(String::from(
                "this process has no Host-issued confirmation channel",
            ))),
        }
    }

    fn require_confirmation_mut(&mut self) -> Result<&mut ConfirmationClient, DesktopError> {
        self.control.as_mut().ok_or_else(|| {
            DesktopError::Protocol(String::from(
                "this process has no Host-issued confirmation channel",
            ))
        })
    }

    #[must_use]
    pub fn confirmation_lost(&self) -> bool {
        self.control
            .as_ref()
            .is_some_and(ConfirmationClient::is_closed)
    }

    pub fn close_after_confirmation_loss(&mut self) {
        self.cancel_secret();
        self.client = None;
        self.pending_pairing = None;
        self.body.shutdown();
        self.body_status = self.body.poll();
    }

    pub async fn reject_pending_challenge(&mut self) {
        let Some(seat) = self.control.as_mut() else {
            return;
        };
        if seat.pending_challenge().is_none() {
            return;
        }
        match seat.reject_pending().await {
            Ok(_) | Err(_) => {}
        }
    }

    pub async fn begin_credential_put(&mut self) -> Result<(), DesktopError> {
        self.require_confirmation()?;
        self.ensure_client()?;
        if self.secret.is_empty() {
            return Err(DesktopError::Protocol(String::from(
                "secret intake is empty",
            )));
        }
        let staged = self.register_credential_intent().await?;
        match staged {
            ManagementOutcome::HeldByOperation | ManagementOutcome::AppliedAsOneTime => {}
            other => {
                return Err(DesktopError::Protocol(format!(
                    "credential intent must stage or apply, got {other:?}"
                )));
            }
        }
        let seat = self.require_confirmation_mut()?;
        let result = seat
            .request_credential_put(SETUP_PROVIDER_OPENAI, session::SETUP_CREDENTIAL_LABEL)
            .await;
        if result.is_err() {
            self.secret.cancel();
        }
        result?;
        self.page = Page::Confirm;
        Ok(())
    }

    pub async fn confirm_owner(&mut self) -> Result<FromConfirmation, DesktopError> {
        let challenge = self
            .control
            .as_ref()
            .and_then(ConfirmationClient::pending_challenge)
            .cloned();
        let deletion_confirm = challenge
            .as_ref()
            .is_some_and(|challenge| matches!(challenge.op, ControlOp::DeletionConfirm));
        let credential_secret = if challenge
            .as_ref()
            .is_some_and(|challenge| matches!(challenge.op, ControlOp::CredentialPut))
        {
            Some(self.secret.take())
        } else {
            None
        };
        let reply = {
            let Self {
                control,
                client,
                deletion,
                timeline,
                history,
                composer,
                memory,
                tasks,
                usage,
                chat_receipt,
                surface_erasure,
                ..
            } = self;
            let seat = control.as_mut().ok_or_else(|| {
                DesktopError::Protocol(String::from(
                    "this process has no Host-issued confirmation channel",
                ))
            })?;
            if deletion_confirm {
                let mut copies = GuiOwned {
                    timeline,
                    history,
                    composer,
                    memory,
                    tasks,
                    usage,
                    deletion,
                    chat_receipt,
                };
                complete_pending_pumping(
                    seat,
                    client.as_mut(),
                    &mut copies,
                    surface_erasure.as_ref(),
                )
                .await?
            } else if let Some(secret) = credential_secret {
                seat.complete_credential(secret).await?
            } else {
                seat.complete_pending().await?
            }
        };
        self.secret.cancel();
        match &reply {
            FromConfirmation::Outcome(ControlOutcome::DeviceApproved { .. }) => {
                let pending = self.pending_pairing.take().ok_or_else(|| {
                    DesktopError::Protocol(String::from(
                        "device approval has no live originating pairing connection",
                    ))
                })?;
                let client = pending.complete().await.map_err(DesktopError::Client)?;
                self.adopt_client(client);
                self.page = Page::Wizard;
            }
            FromConfirmation::Outcome(ControlOutcome::CredentialStored { .. }) => {
                self.page = Page::Wizard;
                self.refresh_setup().await?;
            }
            FromConfirmation::Outcome(ControlOutcome::Deletion(_)) => {
                self.page = Page::Deletion;
                match self.refresh_deletion().await {
                    Ok(()) | Err(_) => {}
                }
            }
            FromConfirmation::Outcome(ControlOutcome::CredentialStaged { .. }) => {
                // Staging is an intermediate step of the intake path; the
                // Owner's surface continues to the completion that follows.
            }
            FromConfirmation::DeniedByBoundary => {
                self.pending_pairing = None;
                self.page = Page::Wizard;
            }
            FromConfirmation::Outcome(
                ControlOutcome::CredentialRefused { .. }
                | ControlOutcome::CredentialUncommitted { .. }
                | ControlOutcome::DeviceUnknown { .. }
                | ControlOutcome::Rejected { .. },
            )
            | FromConfirmation::Unavailable => {
                self.pending_pairing = None;
                self.page = Page::Wizard;
            }
            _ => {}
        }
        self.flush_pending_erasure().await;
        Ok(reply)
    }

    pub async fn register_credential_intent(&mut self) -> Result<ManagementOutcome, DesktopError> {
        self.refresh_setup().await?;
        let mark = self.facts.mark.clone();
        let answer = {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            request_with_timeout(
                client,
                session::credential_intent(&mark, SETUP_PROVIDER_OPENAI),
                DEFAULT_REQUEST_TIMEOUT,
            )
            .await?
        };
        self.flush_pending_erasure().await;
        match answer {
            WirePayload::ManagementOutcome(outcome) => {
                self.refresh_setup().await?;
                Ok(outcome)
            }
            other => Err(DesktopError::Protocol(format!(
                "expected ManagementOutcome, got {}",
                other.message_type()
            ))),
        }
    }

    pub async fn assign_model(&mut self) -> Result<ManagementOutcome, DesktopError> {
        self.refresh_setup().await?;
        let mark = self.facts.mark.clone();
        let model = self.model.clone();
        let assign = {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            request_with_timeout(
                client,
                session::assignment_intent(&mark, SETUP_PROVIDER_OPENAI, &model),
                DEFAULT_REQUEST_TIMEOUT,
            )
            .await?
        };
        self.flush_pending_erasure().await;
        let WirePayload::ManagementOutcome(outcome) = assign else {
            return Err(DesktopError::Protocol(String::from(
                "assignment did not return an outcome",
            )));
        };
        self.refresh_setup().await?;
        if matches!(outcome, ManagementOutcome::StoredAsRuleView { .. }) {
            let mark = self.facts.mark.clone();
            let complete_answer = {
                let client = self.client.as_mut().ok_or_else(|| {
                    DesktopError::Transport(String::from("client is not connected"))
                })?;
                request_with_timeout(
                    client,
                    session::setup_complete_intent(&mark),
                    DEFAULT_REQUEST_TIMEOUT,
                )
                .await?
            };
            self.flush_pending_erasure().await;
            match complete_answer {
                WirePayload::ManagementOutcome(complete) => {
                    if !matches!(
                        complete,
                        ManagementOutcome::AppliedAsOneTime
                            | ManagementOutcome::StoredAsRuleView { .. }
                    ) {
                        return Ok(complete);
                    }
                }
                other => {
                    return Err(DesktopError::Protocol(format!(
                        "complete answered {}",
                        other.message_type()
                    )));
                }
            }
            self.refresh_setup().await?;
            if self.facts.setup_ready() {
                self.page = Page::Chat;
            }
        }
        Ok(outcome)
    }

    pub fn take_client(&mut self) -> Option<Client> {
        self.tasks.reset_connection_state();
        self.client.take()
    }

    pub fn restore_client(&mut self, client: Client) {
        self.adopt_client(client);
    }

    pub async fn send_text(&mut self) -> Result<ChatSendReport, DesktopError> {
        if self.client.is_none() {
            return Ok(ChatSendReport::NotSent { failure: None });
        }
        if self.composer.draft().trim().is_empty() {
            return Ok(ChatSendReport::NoDraft);
        }
        self.project_body_pose(PoseHint::Listening);
        let lang = self.locale.as_tag().to_string();
        let collected = {
            let text = self.composer.draft();
            match self.client.as_mut() {
                Some(client) => session::submit_and_collect(client, text, &lang).await,
                None => {
                    return Ok(ChatSendReport::NotSent { failure: None });
                }
            }
        };
        match collected {
            ChatSessionOutcome::Completed(turn) => {
                self.project_body_pose(PoseHint::Speaking);
                let ChatTurn {
                    round,
                    stream,
                    reply,
                } = turn;
                let Some(text) = self.composer.take_sendable() else {
                    return Ok(ChatSendReport::NotSent { failure: None });
                };
                self.timeline.push(super::presentation::Message {
                    round: round.clone(),
                    owner: true,
                    text,
                    caption: String::new(),
                });
                self.timeline.push(super::presentation::Message {
                    round: round.clone(),
                    owner: false,
                    text: reply,
                    caption: String::new(),
                });
                self.chat_receipt = Some((round.clone(), stream));
                if let Some(client) = self.client.as_mut() {
                    match session::confirm_chat_presentation(
                        client,
                        &round,
                        stream,
                        PresentationStatus::Presented,
                    )
                    .await
                    {
                        Ok(()) | Err(_) => {}
                    }
                }
                let history_result = self.refresh_history().await;
                self.flush_pending_erasure().await;
                match history_result {
                    Ok(()) => Ok(ChatSendReport::Completed),
                    Err(error) => Ok(ChatSendReport::ReplyShownHistoryRefreshFailed {
                        failure: ChatTechnicalFailure {
                            operation: ChatOperation::HistoryRefresh,
                            phase: ChatDeliveryPhase::Accepted,
                            error,
                        },
                    }),
                }
            }
            ChatSessionOutcome::Refused(refusal) => {
                self.project_body_pose(PoseHint::Attention);
                self.flush_pending_erasure().await;
                Ok(ChatSendReport::Refused(refusal))
            }
            ChatSessionOutcome::NotSent(failure) => {
                self.project_body_pose(PoseHint::Attention);
                self.flush_pending_erasure().await;
                Ok(ChatSendReport::NotSent {
                    failure: Some(failure),
                })
            }
            ChatSessionOutcome::OutcomeUnknown(failure) => {
                self.project_body_pose(PoseHint::Attention);
                self.composer.discard_sendable();
                self.flush_pending_erasure().await;
                Ok(ChatSendReport::OutcomeUnknown(failure))
            }
            ChatSessionOutcome::AcceptedFailure { round, failure } => {
                self.project_body_pose(PoseHint::Attention);
                if let Some(text) = self.composer.take_sendable() {
                    self.timeline.push(super::presentation::Message {
                        round: round.clone(),
                        owner: true,
                        text,
                        caption: String::new(),
                    });
                }
                match self.refresh_history().await {
                    Ok(()) | Err(_) => {}
                }
                self.flush_pending_erasure().await;
                Ok(ChatSendReport::AcceptedFailure { round, failure })
            }
            ChatSessionOutcome::StreamEnded { round, end } => {
                self.project_body_pose(PoseHint::Attention);
                if let Some(text) = self.composer.take_sendable() {
                    self.timeline.push(super::presentation::Message {
                        round: round.clone(),
                        owner: true,
                        text,
                        caption: String::new(),
                    });
                }
                match self.refresh_history().await {
                    Ok(()) | Err(_) => {}
                }
                self.flush_pending_erasure().await;
                Ok(ChatSendReport::StreamEnded { round, end })
            }
        }
    }

    pub async fn refresh_history(&mut self) -> Result<(), DesktopError> {
        let history = {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            session::fetch_history(client, session::DEFAULT_HISTORY_LIMIT).await?
        };
        self.history = history;
        self.flush_pending_erasure().await;
        Ok(())
    }

    pub async fn refresh_setup(&mut self) -> Result<(), DesktopError> {
        let view = {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            session::fetch_setup_view(client).await?
        };
        self.facts = SetupFacts::from_view(&view);
        self.flush_pending_erasure().await;
        Ok(())
    }

    #[must_use]
    pub fn memory(&self) -> &MemoryPage {
        &self.memory
    }

    pub async fn refresh_memory(&mut self) -> Result<(), DesktopError> {
        self.fetch_memory(MemoryPage::list_request(None), false)
            .await
    }

    pub async fn page_older_memories(&mut self) -> Result<(), DesktopError> {
        let Some(after) = self.memory.next_after().map(str::to_owned) else {
            return Ok(());
        };
        self.fetch_memory(MemoryPage::list_request(Some(&after)), true)
            .await
    }

    pub async fn open_memory_revisions(&mut self, memory_id: &str) -> Result<(), DesktopError> {
        self.fetch_memory(MemoryPage::revisions_request(memory_id, None), false)
            .await
    }

    pub async fn page_later_revisions(&mut self) -> Result<(), DesktopError> {
        let Some(memory_id) = self.memory.revisions_of().map(str::to_owned) else {
            return Ok(());
        };
        let Some(after) = self.memory.next_revision_after() else {
            return Ok(());
        };
        self.fetch_memory(MemoryPage::revisions_request(&memory_id, Some(after)), true)
            .await
    }

    async fn fetch_memory(
        &mut self,
        view_request: ManagementViewRequest,
        append: bool,
    ) -> Result<(), DesktopError> {
        let view = {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            match request_with_timeout(
                client,
                WirePayload::ManagementViewRequest(view_request.clone()),
                DEFAULT_REQUEST_TIMEOUT,
            )
            .await?
            {
                WirePayload::ManagementView(view) => view,
                other => {
                    return Err(DesktopError::Protocol(format!(
                        "expected ManagementView, got {}",
                        other.message_type()
                    )));
                }
            }
        };
        self.memory.apply_host_view(&view, view_request, append);
        self.flush_pending_erasure().await;
        Ok(())
    }

    pub async fn open_tasks(&mut self) -> Result<(), DesktopError> {
        self.page = Page::Tasks;
        self.refresh_tasks().await
    }

    pub async fn refresh_tasks(&mut self) -> Result<(), DesktopError> {
        let result = {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            self.tasks.refresh_list(client).await
        };
        self.flush_pending_erasure().await;
        result
    }

    pub async fn select_listed_task(&mut self, index: usize) -> Result<(), DesktopError> {
        let result = {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            self.tasks.select(client, index).await
        };
        self.flush_pending_erasure().await;
        result
    }

    pub async fn select_workspace_folder(
        &mut self,
        path: &Path,
    ) -> Result<ManagementOutcome, DesktopError> {
        self.refresh_setup().await?;
        let mark = self.facts.mark.clone();
        let result = {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            self.tasks.select_workspace(client, &mark, path).await
        };
        self.flush_pending_erasure().await;
        result
    }

    pub async fn cancel_displayed_task(&mut self) -> Result<ManagementOutcome, DesktopError> {
        self.refresh_setup().await?;
        let mark = self.facts.mark.clone();
        let result = {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            let outcome = self.tasks.cancel_displayed(client, &mark).await?;
            let _result = self.tasks.refresh_list(client).await;
            Ok(outcome)
        };
        self.flush_pending_erasure().await;
        result
    }

    pub async fn resume_displayed_task(
        &mut self,
        instruction: String,
    ) -> Result<ene_api::v1::undelivered::ResumeTaskOutcomeWire, DesktopError> {
        let result = {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            self.tasks.resume_displayed(client, instruction).await
        };
        self.flush_pending_erasure().await;
        result
    }

    pub async fn present_task_undelivered(&mut self) -> Result<(), DesktopError> {
        let result = {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            self.tasks.present_undelivered(client).await
        };
        self.flush_pending_erasure().await;
        result
    }

    pub async fn ack_presented_tasks(
        &mut self,
    ) -> Result<ene_api::v1::undelivered::UndeliveredAckOutcome, DesktopError> {
        let result = {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            self.tasks.ack_presented(client).await
        };
        self.flush_pending_erasure().await;
        result
    }

    #[must_use]
    pub fn has_presented_task_receipt(&self) -> bool {
        self.tasks.has_presented_receipt()
    }

    pub async fn refresh_usage(&mut self) -> Result<(), DesktopError> {
        {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            self.usage.refresh(client).await?;
        }
        self.flush_pending_erasure().await;
        Ok(())
    }

    pub async fn next_usage_page(&mut self) -> Result<(), DesktopError> {
        {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            self.usage.next_page(client).await?;
        }
        self.flush_pending_erasure().await;
        Ok(())
    }

    pub fn set_usage_cap_limit_micros(&mut self, micros: u64) {
        self.usage.set_cap_limit_micros(micros);
    }

    pub async fn apply_usage_cap(&mut self) -> Result<ManagementOutcome, DesktopError> {
        let outcome = {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            self.usage.apply_cap(client).await?
        };
        self.flush_pending_erasure().await;
        Ok(outcome)
    }

    pub fn set_deletion_exact_text(&mut self, text: String) {
        self.deletion.set_exact_text(text);
    }

    pub async fn request_deletion(&mut self) -> Result<ManagementOutcome, DesktopError> {
        let outcome = {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            self.deletion.request(client).await?
        };
        self.page = Page::Deletion;
        self.flush_pending_erasure().await;
        Ok(outcome)
    }

    pub async fn refresh_deletion(&mut self) -> Result<(), DesktopError> {
        {
            let client = self
                .client
                .as_mut()
                .ok_or_else(|| DesktopError::Transport(String::from("client is not connected")))?;
            self.deletion.refresh(client).await?;
        }
        self.flush_pending_erasure().await;
        Ok(())
    }

    pub async fn begin_deletion_confirm(&mut self) -> Result<(), DesktopError> {
        let Self {
            control, deletion, ..
        } = self;
        let seat = control.as_mut().ok_or_else(|| {
            DesktopError::Protocol(String::from(
                "this process has no Host-issued confirmation channel",
            ))
        })?;
        deletion.begin_confirm(seat).await?;
        self.page = Page::Confirm;
        Ok(())
    }

    pub async fn resume_deletion(&mut self) -> Result<FromConfirmation, DesktopError> {
        let Self {
            control,
            client,
            deletion,
            timeline,
            history,
            composer,
            memory,
            tasks,
            usage,
            chat_receipt,
            surface_erasure,
            ..
        } = self;
        let reply = {
            let seat = control.as_mut().ok_or_else(|| {
                DesktopError::Protocol(String::from(
                    "this process has no Host-issued confirmation channel",
                ))
            })?;
            let (operation, sweep) = deletion.resume_target()?;
            let resume = seat.request_deletion_resume(&operation, sweep);
            match client.as_mut() {
                Some(client) => {
                    let mut copies = GuiOwned {
                        timeline,
                        history,
                        composer,
                        memory,
                        tasks,
                        usage,
                        deletion,
                        chat_receipt,
                    };
                    let reply =
                        pump_while_pending(resume, client, &mut copies, surface_erasure.as_ref())
                            .await?;
                    copies.deletion.note_resume(&reply);
                    reply
                }
                None => {
                    let reply = resume.await?;
                    deletion.note_resume(&reply);
                    reply
                }
            }
        };
        match self.refresh_deletion().await {
            Ok(()) | Err(_) => {}
        }
        Ok(reply)
    }

    #[must_use]
    pub fn has_chat_receipt(&self) -> bool {
        self.chat_receipt.is_some()
    }

    fn adopt_client(&mut self, mut client: Client) {
        client.defer_erasure();
        self.client = Some(client);
    }

    async fn refresh_after_connect(&mut self) {
        if self.refresh_setup().await.is_ok() && self.facts.setup_ready() {
            self.page = Page::Chat;
        }
        match self.refresh_history().await {
            Ok(()) | Err(_) => {}
        }
        match self.refresh_tasks().await {
            Ok(()) | Err(_) => {}
        }
    }

    async fn flush_pending_erasure(&mut self) {
        let Some(client) = self.client.as_mut() else {
            return;
        };
        let mut copies = GuiOwned {
            timeline: &mut self.timeline,
            history: &mut self.history,
            composer: &mut self.composer,
            memory: &mut self.memory,
            tasks: &mut self.tasks,
            usage: &mut self.usage,
            deletion: &mut self.deletion,
            chat_receipt: &mut self.chat_receipt,
        };
        apply_pending_erasure(client, &mut copies, self.surface_erasure.as_ref()).await;
    }

    fn ensure_client(&self) -> Result<(), DesktopError> {
        if self.client.is_some() {
            Ok(())
        } else {
            Err(DesktopError::Transport(String::from(
                "client is not connected",
            )))
        }
    }
}

async fn apply_pending_erasure(
    client: &mut Client,
    copies: &mut GuiOwned<'_>,
    surface: Option<&super::presentation::SurfaceErasure>,
) {
    loop {
        let Some(demand) = client.take_pending_erasure() else {
            return;
        };
        let mut result = erasure::apply_demand(&demand, copies);
        if let Some(erase) = surface
            && !erase().await
        {
            result.unverified.append(&mut result.wiped);
        }
        match client.report_local_erasure(result).await {
            Ok(()) | Err(_) => {}
        }
    }
}

async fn complete_pending_pumping(
    seat: &mut ConfirmationClient,
    client: Option<&mut Client>,
    copies: &mut GuiOwned<'_>,
    surface: Option<&super::presentation::SurfaceErasure>,
) -> Result<FromConfirmation, DesktopError> {
    let Some(client) = client else {
        return seat.complete_pending().await;
    };
    let complete = seat.complete_pending();
    pump_while_pending(complete, client, copies, surface).await
}

async fn pump_while_pending<F>(
    pump: F,
    client: &mut Client,
    copies: &mut GuiOwned<'_>,
    surface: Option<&super::presentation::SurfaceErasure>,
) -> F::Output
where
    F: std::future::Future,
{
    tokio::pin!(pump);
    loop {
        tokio::select! {
            outcome = &mut pump => return outcome,
            () = tokio::time::sleep(Duration::from_millis(20)) => {
                match copies.deletion.refresh(client).await {
                    Ok(()) | Err(_) => {}
                }
                apply_pending_erasure(client, copies, surface).await;
            }
        }
    }
}

fn locale_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LOCALE_FILE)
}

fn load_locale(data_dir: &Path) -> Locale {
    std::fs::read_to_string(locale_path(data_dir))
        .ok()
        .map(|text| Locale::parse(text.trim()))
        .unwrap_or(Locale::Ja)
}

fn persist_locale(data_dir: &Path, locale: Locale) {
    match std::fs::write(locale_path(data_dir), locale.as_tag()) {
        Ok(()) | Err(_) => {}
    }
}

impl DesktopRuntime {
    pub fn attach_surface_erasure(&mut self, erase: super::presentation::SurfaceErasure) {
        self.surface_erasure = Some(erase);
    }
    pub fn surface_snapshot(&self) -> super::presentation::SurfaceSnapshot {
        use super::presentation::{Confirmation, Message, Row, SurfaceSnapshot, memory_key, tr};
        let locale = self.locale;
        let mut messages: Vec<Message> = self
            .history
            .iter()
            .map(|h| Message {
                round: h.round.0.clone(),
                owner: matches!(h.role, ene_api::v1::round::HistoryRole::Owner),
                text: h.text.clone(),
                caption: h.at.clone(),
            })
            .collect();
        messages.extend(
            self.timeline
                .iter()
                .filter(|m| {
                    !self.history.iter().any(|h| {
                        h.round.0 == m.round
                            && matches!(h.role, ene_api::v1::round::HistoryRole::Owner) == m.owner
                    })
                })
                .cloned(),
        );
        SurfaceSnapshot {
            japanese: locale == Locale::Ja, connected: self.client.is_some(), ready: self.facts.setup_ready(),
            credential: self.facts.credential_present, consent: self.facts.consent_assigned,
            assigned_model: self.facts.model.clone().unwrap_or_else(|| String::from(DEFAULT_MODEL)),
            step: match self.wizard_step { WizardStep::Language => 0, WizardStep::BundledEne => 1, WizardStep::CloudCost => 2, WizardStep::Credential => 3, WizardStep::Assignment => 4 },
            status: if self.client.is_some() { tr(locale, "接続済み", "Connected") } else { tr(locale, "未接続 · セットアップを確認してください", "Disconnected · review setup") },
            body_available: self.body_status == BodyStatus::Spawned && self.body.available(),
            body_visible: self.body_status == BodyStatus::Spawned
                && self.body.available()
                && !self.body_hidden,
            messages, tasks: self.tasks.rows(locale), details: self.tasks.details(locale), selected_task: self.tasks.selected_key(),
            memories: self.memory.rows().iter().map(|m| Row { key: memory_key(&m.id, &m.revision), title: tr(locale, "記憶", "Memory"), body: m.content.clone(), meta: m.created_at.clone(), state: m.importance.clone() }).collect(),
            revisions: self.memory.revisions().iter().map(|m| Row { title: format!("{} {}", tr(locale, "履歴", "Revision"), m.revision), body: m.content.clone(), meta: [m.at.clone(), m.grounds_summary.clone().unwrap_or_default(), m.grounds.clone().unwrap_or_default()].join("
"), ..Row::default() }).collect(),
            selected_memory: self.memory.revisions_of().and_then(|id| self.memory.rows().iter().find(|m| m.id == id)).map(|m| memory_key(&m.id, &m.revision)).unwrap_or_default(),
            memory_more: self.memory.next_after().is_some(), revisions_more: self.memory.next_revision_after().is_some(),
            usage: self.usage.rows(locale), caps: self.usage.cap_rows(locale), usage_more: self.usage.has_more(),
            deletions: self.deletion.rows(locale), selected_deletion: self.deletion.selected_key(), can_resume_deletion: self.deletion.can_resume(),
            confirmation: self.control.as_ref().and_then(ConfirmationClient::pending_challenge).map(|c| {
                let (ja, en, ja_desc, en_desc, target) = match c.op {
                    ControlOp::DeviceApprove => ("この端末を接続", "Connect this device", "この端末から会話と管理を行えるようにします。", "Allow this device to use conversation and management.", tr(locale, "このデスクトップ端末", "This desktop device")),
                    ControlOp::CredentialPut => ("API キーを登録", "Register API key", "入力したキーを Host の資格情報ストアへ登録します。", "Store the entered key in the Host credential store.", String::from("OpenAI")),
                    ControlOp::DeletionConfirm => ("データ削除を確認", "Confirm data deletion", "選択した削除要求を実行します。対象データと一時的な表示コピーが削除されます。", "Execute the selected deletion request, including matching data and temporary display copies.", tr(locale, "選択した削除要求", "Selected deletion request")),
                };
                Confirmation { key: c.session_id.to_string(), title: tr(locale, ja, en), description: tr(locale, ja_desc, en_desc), target }
            }),
        }
    }
    pub async fn select_task_key(&mut self, key: &str) -> Result<(), DesktopError> {
        let index = self
            .tasks
            .index_for_key(key)
            .ok_or_else(|| DesktopError::Protocol(String::from("stale task selection")))?;
        self.select_listed_task(index).await
    }
    pub fn check_task_key(&self, key: &str) -> Result<(), DesktopError> {
        if !key.is_empty() && self.tasks.selected_key() == key {
            Ok(())
        } else {
            Err(DesktopError::Protocol(String::from("stale task selection")))
        }
    }
    pub async fn select_memory_key(&mut self, key: &str) -> Result<(), DesktopError> {
        let id = self
            .memory
            .rows()
            .iter()
            .find(|m| super::presentation::memory_key(&m.id, &m.revision) == key)
            .map(|m| m.id.clone())
            .ok_or_else(|| DesktopError::Protocol(String::from("stale memory selection")))?;
        self.open_memory_revisions(&id).await
    }
    pub fn select_deletion_key(&mut self, key: &str) -> Result<(), DesktopError> {
        self.deletion.select_key(key)
    }
    pub async fn refresh_deletion_requests(&mut self) -> Result<(), DesktopError> {
        self.refresh_deletion().await?;
        let seat = self
            .control
            .as_mut()
            .ok_or(DesktopError::DeniedByBoundary)?;
        self.deletion.refresh_pending(seat).await
    }
    pub async fn confirm_key(&mut self, key: &str) -> Result<FromConfirmation, DesktopError> {
        if !self
            .control
            .as_ref()
            .and_then(ConfirmationClient::pending_challenge)
            .is_some_and(|c| c.session_id.to_string() == key)
        {
            return Err(DesktopError::Protocol(String::from("stale confirmation")));
        }
        self.confirm_owner().await
    }
}

#[cfg(test)]
mod tests {
    use super::DesktopRuntime;
    use crate::session::ChatSendReport;

    #[tokio::test]
    async fn disconnected_send_keeps_the_typed_text_in_the_draft() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut runtime = DesktopRuntime::new(dir.path().to_path_buf());
        runtime.composer_mut().set_draft(String::from("keep me"));
        let result = runtime.send_text().await;
        assert!(
            matches!(result, Ok(ChatSendReport::NotSent { failure: None })),
            "a disconnected send must report the no-client outcome: {result:?}"
        );
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.draft, "keep me");
        assert!(snapshot.timeline.is_empty());
    }
}
