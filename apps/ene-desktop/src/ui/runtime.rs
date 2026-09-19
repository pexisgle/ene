//! Testable desktop runtime. Host I/O never runs inside [`DesktopRuntime::tick`].

use std::path::{Path, PathBuf};

use ene_api::v1::management::ManagementOutcome;
use ene_api::v1::payload::WirePayload;
use ene_api::v1::round::HistoryItem;
use ene_client::Client;
use ene_local_control::{ControlOutcome, FromHost};
use zeroize::Zeroize as _;

use crate::body_supervise::{BodyStatus, BodySupervisor};
use crate::control::ControlSeat;
use crate::host_launch::{self, DetachedHost};
use crate::i18n::{self, Label, Locale};
use crate::secret::SecretIntake;
use crate::session::{self, SETUP_PROVIDER_OPENAI, SetupFacts};
use crate::ui::{Composer, DesktopError, GuiSnapshot, Page, WizardStep, history_lines};
use crate::{BUNDLED_ENE_ASSET, DESKTOP_DESCRIPTOR};

const LOCALE_FILE: &str = "desktop-locale";
const DEFAULT_MODEL: &str = "gpt-4.1";

/// Headless first-party desktop: the same state the Slint window projects.
pub struct DesktopRuntime {
    data_dir: PathBuf,
    locale: Locale,
    page: Page,
    wizard_step: WizardStep,
    composer: Composer,
    secret: SecretIntake,
    client: Option<Client>,
    control: Option<ControlSeat>,
    timeline: Vec<String>,
    history: Vec<HistoryItem>,
    connection: &'static str,
    presence: String,
    deny_reason: String,
    facts: SetupFacts,
    ui_ticks: u64,
    body: BodySupervisor,
    body_status: BodyStatus,
    model: String,
    detached_host: Option<DetachedHost>,
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
            control: None,
            timeline: Vec::new(),
            history: Vec::new(),
            connection: i18n::label(locale, Label::Disconnected),
            presence: String::from("unknown"),
            deny_reason: String::new(),
            facts: SetupFacts {
                credential_present: false,
                consent_assigned: false,
                model: None,
                mark: String::new(),
            },
            ui_ticks: 0,
            body: BodySupervisor::new(),
            body_status: BodyStatus::Absent,
            model: String::from(DEFAULT_MODEL),
            detached_host: None,
        }
    }

    /// GUI event-loop pump. Never waits on connect or provider I/O.
    pub fn tick(&mut self) {
        self.ui_ticks = self.ui_ticks.saturating_add(1);
        self.body_status = self.body.poll();
    }

    #[must_use]
    pub fn ui_ticks(&self) -> u64 {
        self.ui_ticks
    }

    #[must_use]
    pub fn snapshot(&self) -> GuiSnapshot {
        GuiSnapshot {
            locale: self.locale.as_tag().to_string(),
            page: format!("{:?}", self.page),
            wizard_step: format!("{:?}", self.wizard_step),
            timeline: self.timeline.clone(),
            history: history_lines(&self.history),
            draft: self.composer.draft().to_string(),
            composing: self.composer.composing(),
            connection: self.connection.to_string(),
            presence: self
                .client
                .as_ref()
                .and_then(Client::presence_state)
                .map(session::presence_label)
                .map(str::to_string)
                .unwrap_or_else(|| self.presence.clone()),
            deny_reason: self.deny_reason.clone(),
            challenge_target: self
                .control
                .as_ref()
                .and_then(ControlSeat::pending_challenge)
                .map(|challenge| challenge.display_target().to_string()),
            about_slint: true,
            body_status: format!("{:?}", self.body_status),
            ui_ticks: self.ui_ticks,
            setup_ready: self.facts.setup_ready(),
            credential_present: self.facts.credential_present,
            consent_assigned: self.facts.consent_assigned,
            secret_visible: matches!(self.wizard_step, WizardStep::Credential),
            wizard_body: i18n::label(self.locale, wizard_label(self.wizard_step)).to_string(),
        }
    }

    pub fn set_locale(&mut self, locale: Locale) {
        self.locale = locale;
        persist_locale(&self.data_dir, locale);
        self.connection = match self.client.is_some() {
            true => i18n::label(locale, Label::Connected),
            false => i18n::label(locale, Label::Disconnected),
        };
    }

    pub fn open_page(&mut self, page: Page) {
        self.page = page;
        if matches!(page, Page::About) {
            self.tick();
        }
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

    pub fn cancel_secret(&mut self) {
        self.secret.cancel();
        if matches!(self.page, Page::Confirm) {
            self.page = Page::Wizard;
        }
    }

    pub fn set_model(&mut self, model: String) {
        self.model = model;
    }

    pub fn try_spawn_body(&mut self, exe: &Path) {
        self.body_status = self.body.spawn_if_present(exe);
    }

    pub fn bundled_ene_asset(&self) -> PathBuf {
        self.data_dir.join(BUNDLED_ENE_ASSET)
    }

    /// Detach `ene-core serve` when the Client listener is down.
    pub fn ensure_host(&mut self, host_bin: Option<&Path>) -> Result<(), DesktopError> {
        if host_launch::host_is_serving(&self.data_dir) {
            return Ok(());
        }
        let binary = match host_bin
            .map(Path::to_path_buf)
            .or_else(host_launch::locate_host_binary)
        {
            Some(path) => path,
            None => {
                return Err(DesktopError::HostLaunch(String::from(
                    "ene-core binary was not found",
                )));
            }
        };
        let detached = host_launch::detach_serve(&self.data_dir, &binary)
            .map_err(|error| DesktopError::HostLaunch(error.to_string()))?;
        if detached.pid == 0 {
            return Err(DesktopError::HostLaunch(String::from(
                "detached host reported pid 0",
            )));
        }
        self.detached_host = Some(detached);
        Ok(())
    }

    pub async fn occupy_seat(&mut self) -> Result<(), DesktopError> {
        if self.control.is_some() {
            return Ok(());
        }
        self.control = Some(ControlSeat::occupy(&self.data_dir).await?);
        Ok(())
    }

    /// Start pairing; leaves a confirmation challenge for the seated owner.
    pub async fn begin_pairing(&mut self) -> Result<(), DesktopError> {
        self.occupy_seat().await?;
        self.connection = i18n::label(self.locale, Label::Connecting);
        let pending = session::connect_until_pending(&self.data_dir, DESKTOP_DESCRIPTOR).await?;
        let seat = self
            .control
            .as_mut()
            .ok_or(DesktopError::DeniedByBoundary)?;
        seat.request_device_approve(&pending).await?;
        self.page = Page::Confirm;
        Ok(())
    }

    pub async fn begin_credential_put(&mut self) -> Result<(), DesktopError> {
        self.occupy_seat().await?;
        self.ensure_client()?;
        // Wire intent stages the pending pair. Control put+approve then
        // makes it usable. Approve without a pending does not create the ref.
        let staged = self.register_credential_intent().await?;
        match staged {
            ManagementOutcome::HeldByOperation | ManagementOutcome::AppliedAsOneTime => {}
            other => {
                return Err(DesktopError::Protocol(format!(
                    "credential intent must stage or apply, got {other:?}"
                )));
            }
        }
        if self.secret.is_empty() {
            return Err(DesktopError::Protocol(String::from(
                "secret intake is empty",
            )));
        }
        let secret = self.secret.take();
        let seat = self
            .control
            .as_mut()
            .ok_or(DesktopError::DeniedByBoundary)?;
        let result = seat
            .request_credential_put(
                SETUP_PROVIDER_OPENAI,
                session::SETUP_CREDENTIAL_LABEL,
                secret,
            )
            .await;
        if result.is_err() {
            self.secret.cancel();
        }
        result?;
        self.page = Page::Confirm;
        Ok(())
    }

    /// Owner gesture on the seated confirmation surface.
    pub async fn confirm_owner(&mut self) -> Result<FromHost, DesktopError> {
        let seat = self
            .control
            .as_mut()
            .ok_or(DesktopError::DeniedByBoundary)?;
        let reply = seat.complete_pending().await?;
        self.secret.cancel();
        match &reply {
            FromHost::Outcome(ControlOutcome::DeviceApproved { pairing_secret, .. }) => {
                let mut secret = pairing_secret.clone().into_inner();
                match session::connect(&self.data_dir, DESKTOP_DESCRIPTOR, Some(secret.clone()))
                    .await
                {
                    Ok(client) => {
                        self.client = Some(client);
                        self.connection = i18n::label(self.locale, Label::Connected);
                        self.pull_presence();
                    }
                    Err(error) => {
                        secret.zeroize();
                        return Err(DesktopError::Client(error));
                    }
                }
                secret.zeroize();
                self.page = Page::Wizard;
            }
            FromHost::Outcome(ControlOutcome::CredentialStored { .. }) => {
                self.page = Page::Wizard;
                self.refresh_setup().await?;
            }
            FromHost::DeniedByBoundary => {
                self.deny_reason = i18n::control_deny(self.locale, &reply);
                self.page = Page::Wizard;
            }
            other => {
                self.deny_reason = i18n::control_deny(self.locale, other);
            }
        }
        Ok(reply)
    }

    pub async fn send_confirmed_true_on_control(&mut self) -> Result<FromHost, DesktopError> {
        let seat = self
            .control
            .as_mut()
            .ok_or(DesktopError::DeniedByBoundary)?;
        let reply = seat.send_confirmed_true().await?;
        if matches!(reply, FromHost::DeniedByBoundary) {
            self.deny_reason = i18n::control_deny(self.locale, &reply);
        }
        Ok(reply)
    }

    pub async fn register_credential_intent(&mut self) -> Result<ManagementOutcome, DesktopError> {
        self.ensure_client()?;
        self.refresh_setup().await?;
        let mark = self.facts.mark.clone();
        let client = self.client.as_mut().ok_or(DesktopError::DeniedByBoundary)?;
        match request(
            client,
            session::credential_intent(&mark, SETUP_PROVIDER_OPENAI),
        )
        .await?
        {
            WirePayload::ManagementOutcome(outcome) => {
                self.deny_reason = i18n::management_deny(self.locale, &outcome);
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
        self.ensure_client()?;
        self.refresh_setup().await?;
        let mark = self.facts.mark.clone();
        let model = self.model.clone();
        let client = self.client.as_mut().ok_or(DesktopError::DeniedByBoundary)?;
        let assign = request(
            client,
            session::assignment_intent(&mark, SETUP_PROVIDER_OPENAI, &model),
        )
        .await?;
        let WirePayload::ManagementOutcome(outcome) = assign else {
            return Err(DesktopError::Protocol(String::from(
                "assignment did not return an outcome",
            )));
        };
        self.deny_reason = i18n::management_deny(self.locale, &outcome);
        self.refresh_setup().await?;
        if matches!(outcome, ManagementOutcome::StoredAsRuleView { .. }) {
            let mark = self.facts.mark.clone();
            let client = self.client.as_mut().ok_or(DesktopError::DeniedByBoundary)?;
            match request(client, session::setup_complete_intent(&mark)).await? {
                WirePayload::ManagementOutcome(complete) => {
                    self.deny_reason = i18n::management_deny(self.locale, &complete);
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

    pub async fn client_confirmed_true(&mut self) -> Result<ManagementOutcome, DesktopError> {
        self.ensure_client()?;
        self.refresh_setup().await?;
        let mark = self.facts.mark.clone();
        let client = self.client.as_mut().ok_or(DesktopError::DeniedByBoundary)?;
        match request(client, session::confirmed_true_intent(&mark)).await? {
            WirePayload::ManagementOutcome(outcome) => {
                self.deny_reason = i18n::management_deny(self.locale, &outcome);
                Ok(outcome)
            }
            other => Err(DesktopError::Protocol(format!(
                "expected ManagementOutcome, got {}",
                other.message_type()
            ))),
        }
    }

    pub fn take_client(&mut self) -> Option<Client> {
        self.connection = i18n::label(self.locale, Label::Disconnected);
        self.presence = String::from("unknown");
        self.client.take()
    }

    pub fn restore_client(&mut self, client: Client) {
        self.client = Some(client);
        self.connection = i18n::label(self.locale, Label::Connected);
        self.pull_presence();
    }

    pub async fn reconnect(&mut self) -> Result<(), DesktopError> {
        self.client = None;
        let client = session::connect(&self.data_dir, DESKTOP_DESCRIPTOR, None)
            .await
            .map_err(DesktopError::Client)?;
        self.client = Some(client);
        self.connection = i18n::label(self.locale, Label::Connected);
        self.pull_presence();
        self.refresh_setup().await?;
        self.refresh_history().await?;
        Ok(())
    }

    pub async fn send_text(&mut self) -> Result<(), DesktopError> {
        let Some(text) = self.composer.take_sendable() else {
            return Ok(());
        };
        self.ensure_client()?;
        let lang = self.locale.as_tag().to_string();
        let client = self.client.as_mut().ok_or(DesktopError::DeniedByBoundary)?;
        match session::submit_and_collect(client, &text, &lang).await {
            Ok(turn) => {
                self.timeline.push(format!("[owner] {text}"));
                self.timeline.push(format!("[companion] {}", turn.reply));
                self.pull_presence();
                self.refresh_history().await?;
                Ok(())
            }
            Err(error) => {
                self.deny_reason = error.to_string();
                self.timeline.push(format!("[owner] {text}"));
                Err(error)
            }
        }
    }

    pub async fn refresh_history(&mut self) -> Result<(), DesktopError> {
        self.ensure_client()?;
        let client = self.client.as_mut().ok_or(DesktopError::DeniedByBoundary)?;
        self.history = session::fetch_history(client, session::DEFAULT_HISTORY_LIMIT).await?;
        Ok(())
    }

    pub async fn refresh_setup(&mut self) -> Result<(), DesktopError> {
        self.ensure_client()?;
        let client = self.client.as_mut().ok_or(DesktopError::DeniedByBoundary)?;
        let view = session::fetch_setup_view(client).await?;
        self.facts = SetupFacts::from_view(&view);
        self.pull_presence();
        Ok(())
    }

    pub async fn refresh_management_without_body(&mut self) -> Result<(), DesktopError> {
        self.body_status = BodyStatus::Absent;
        self.refresh_setup().await
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

    fn pull_presence(&mut self) {
        self.presence = self
            .client
            .as_ref()
            .and_then(Client::presence_state)
            .map(session::presence_label)
            .unwrap_or("unknown")
            .to_string();
    }
}

fn wizard_label(step: WizardStep) -> Label {
    match step {
        WizardStep::Language => Label::WizardLanguage,
        WizardStep::BundledEne => Label::WizardBundledEne,
        WizardStep::CloudCost => Label::WizardCloudCost,
        WizardStep::Credential => Label::WizardCredential,
        WizardStep::Assignment => Label::WizardAssignment,
    }
}

async fn request(client: &mut Client, payload: WirePayload) -> Result<WirePayload, DesktopError> {
    tokio::time::timeout(std::time::Duration::from_secs(15), client.request(payload))
        .await
        .map_err(|_| DesktopError::Transport(String::from("client request timed out")))?
        .map_err(DesktopError::Client)
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
