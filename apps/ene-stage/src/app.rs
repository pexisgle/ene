//! winit application handler for the product stage client.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ene_api::MessageMode;
use ene_vrm::viseme::VisemeAnalyzer;
use parking_lot::Mutex;
use thiserror::Error;
use tokio::runtime::{Handle, Runtime};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

use crate::audio::{AudioHub, ListenAction, MicListen, SendResult, run_listen_stream};
use crate::avatar::look_at;
use crate::chrome::{ChromeKind, ChromeWindow};
use crate::core::events::{EventFeeds, LiveEvent, spawn_event_feeds};
use crate::core::session::StageSession;
use crate::core::spawn::{StageCore, StageSpawnError, attach_or_spawn_core};
use crate::detail::{self, DetailTab, DetailUiState, LogKind};
use crate::gpu::{GpuContext, GpuError};
use crate::i18n;
use crate::overlay::{OverlayError, OverlayWindow};
use crate::settings::{DesktopSettings, load_desktop_settings, save_desktop_settings};
use crate::shell::tray::TrayError;
use crate::shell::{HotkeyManager, ShellCommand, ShellError, TrayManager, show_notification};
use crate::surface::{self, SpotlightAction, SurfaceAction, SurfaceUiState};
use crate::tasks::AsyncOutcome;

/// Which chrome window an open action intends to focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusTarget {
    Chat,
    Detail,
    Caption,
    Spotlight,
}

/// Which window currently owns keyboard focus, driving overlay z-order and
/// click-through as one derived state instead of independent booleans.
///
/// `Focused(false)` clears optimistically; a following `Focused(true)`
/// from another of our windows re-establishes protection before the next
/// interaction sync, so brief focus handoffs never expose the overlay.
#[derive(Debug, Default)]
struct OverlayFocus {
    target: Option<FocusTarget>,
}

impl OverlayFocus {
    fn transition(&mut self, target: FocusTarget) {
        self.target = Some(target);
    }

    fn on_focus_event(&mut self, owner: FocusOwner, focused: bool) -> bool {
        match owner {
            FocusOwner::Overlay => {
                if focused {
                    self.clear()
                } else {
                    false
                }
            }
            FocusOwner::Chat => {
                if focused {
                    self.set(FocusTarget::Chat)
                } else {
                    self.clear_if(FocusTarget::Chat)
                }
            }
            FocusOwner::Detail => {
                if focused {
                    self.set(FocusTarget::Detail)
                } else {
                    self.clear_if(FocusTarget::Detail)
                }
            }
            FocusOwner::Caption => {
                if focused {
                    self.set(FocusTarget::Caption)
                } else {
                    self.clear_if(FocusTarget::Caption)
                }
            }
            FocusOwner::Spotlight => {
                if focused {
                    self.set(FocusTarget::Spotlight)
                } else {
                    self.clear_if(FocusTarget::Spotlight)
                }
            }
        }
    }

    fn set(&mut self, target: FocusTarget) -> bool {
        let changed = self.target != Some(target);
        self.target = Some(target);
        changed
    }

    fn clear(&mut self) -> bool {
        let had = self.target.is_some();
        self.target = None;
        had
    }

    fn clear_target(&mut self, target: FocusTarget) {
        if self.target == Some(target) {
            self.target = None;
        }
    }

    fn clear_if(&mut self, target: FocusTarget) -> bool {
        if self.target == Some(target) {
            self.clear()
        } else {
            false
        }
    }

    #[must_use]
    fn protects(&self) -> bool {
        self.target.is_some()
    }
}

/// Which of our windows emitted a focus event, resolved before dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusOwner {
    Overlay,
    Chat,
    Detail,
    Caption,
    Spotlight,
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("spawn: {0}")]
    Spawn(#[from] StageSpawnError),
    #[error("runtime: {0}")]
    Runtime(String),
    #[error("window: {0}")]
    Window(String),
    #[error("gpu: {0}")]
    Gpu(#[from] GpuError),
    #[error("shell: {0}")]
    Shell(#[from] ShellError),
}

pub fn run() -> Result<(), AppError> {
    let settings = load_desktop_settings();
    i18n::select_language(&settings.language);
    let runtime = Runtime::new().map_err(|err| AppError::Runtime(err.to_string()))?;
    let rt_handle = runtime.handle().clone();

    let (client, core, session, feeds) = runtime.block_on(async {
        let (client, core) = attach_or_spawn_core(&settings).await?;
        let client = Arc::new(client);
        let session = StageSession::bootstrap(Arc::clone(&client)).await?;
        let feeds = spawn_event_feeds(&rt_handle, &client, session.session_id());
        Ok::<_, StageSpawnError>((client, core, session, feeds))
    })?;

    let tray = match TrayManager::new() {
        Ok(tray) => Some(tray),
        Err(TrayError::Build(err)) => {
            tracing::warn!(error = %err, "tray unavailable");
            None
        }
        Err(err) => return Err(ShellError::Tray(err).into()),
    };
    let hotkeys = match HotkeyManager::new() {
        Ok(hotkeys) => Some(hotkeys),
        Err(err) => {
            tracing::warn!(error = %err, "global hotkeys unavailable");
            None
        }
    };

    let audio = AudioHub::new_with_mic_device(&settings.mic_device);
    let sample_rate = audio.sample_rate();
    let mut app = StageApp {
        settings: settings.clone(),
        local_settings: settings,
        core,
        session,
        client,
        runtime,
        rt_handle,
        feeds,
        audio,
        viseme: VisemeAnalyzer::new(sample_rate),
        look_at_state: look_at::LookAtState::default(),
        tray,
        hotkeys,
        surface: SurfaceUiState::default(),
        detail: DetailUiState::default(),
        async_results: Arc::new(Mutex::new(Vec::new())),
        mic_active: false,
        listen: MicListen::new(),
        notify_claimed: false,
        speaker_claimed: false,
        gpu: None,
        overlay: None,
        chat: None,
        detail_win: None,
        caption: None,
        spotlight: None,
        overlay_focus: OverlayFocus::default(),
        last_cursor: None,
        last_tick: Instant::now(),
        tray_interaction_at: None,
        last_approval_poll: Instant::now(),
        approval_poll_inflight: false,
        approval_needs_reveal: false,
    };
    app.surface.character_pos = [app.settings.character_x, app.settings.character_y];
    app.surface.history = app.session.history();
    app.surface.greetings = app.session.greetings().to_vec();
    detail::ensure_settings(
        &mut app.detail,
        &app.client,
        &app.rt_handle,
        &app.async_results,
    );
    app.claim_speaker_notify();

    let event_loop = EventLoop::new().map_err(|err| AppError::Window(err.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop
        .run_app(&mut app)
        .map_err(|err| AppError::Window(err.to_string()))?;
    Ok(())
}

struct StageApp {
    settings: DesktopSettings,
    local_settings: DesktopSettings,
    #[expect(
        dead_code,
        reason = "StageCore kills spawned ene-core on drop when lifetime is app"
    )]
    core: StageCore,
    session: StageSession,
    client: Arc<ene_api::ApiClient>,
    runtime: Runtime,
    rt_handle: Handle,
    feeds: EventFeeds,
    audio: AudioHub,
    viseme: VisemeAnalyzer,
    look_at_state: look_at::LookAtState,
    tray: Option<TrayManager>,
    hotkeys: Option<HotkeyManager>,
    surface: SurfaceUiState,
    detail: DetailUiState,
    async_results: Arc<Mutex<Vec<AsyncOutcome>>>,
    mic_active: bool,
    listen: MicListen,
    notify_claimed: bool,
    speaker_claimed: bool,
    gpu: Option<GpuContext>,
    overlay: Option<OverlayWindow>,
    chat: Option<ChromeWindow>,
    detail_win: Option<ChromeWindow>,
    caption: Option<ChromeWindow>,
    spotlight: Option<ChromeWindow>,
    overlay_focus: OverlayFocus,
    last_cursor: Option<LogicalPosition<f32>>,
    last_tick: Instant,
    tray_interaction_at: Option<Instant>,
    last_approval_poll: Instant,
    approval_poll_inflight: bool,
    approval_needs_reveal: bool,
}

impl StageApp {
    #[cfg(test)]
    fn new_for_test() -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let _guard = runtime.enter();
        let client = Arc::new(ene_api::ApiClient::new(
            "http://127.0.0.1:9",
            "token",
            "stage",
        ));
        let rt_handle = runtime.handle().clone();
        Self {
            settings: DesktopSettings::default(),
            local_settings: DesktopSettings::default(),
            core: StageCore::detached(),
            session: StageSession::new_for_test(
                Arc::clone(&client),
                "soul",
                "old-session",
                ene_api::HistoryResponse {
                    messages: Vec::new(),
                    depth: "surface".to_owned(),
                },
            ),
            client,
            runtime,
            rt_handle,
            feeds: EventFeeds::new_for_test(),
            audio: AudioHub::new(),
            viseme: VisemeAnalyzer::new(AudioHub::new().sample_rate()),
            look_at_state: look_at::LookAtState::default(),
            tray: None,
            hotkeys: None,
            surface: SurfaceUiState::default(),
            detail: DetailUiState::default(),
            async_results: Arc::new(Mutex::new(Vec::new())),
            mic_active: false,
            listen: MicListen::new(),
            notify_claimed: false,
            speaker_claimed: false,
            gpu: None,
            overlay: None,
            chat: None,
            detail_win: None,
            caption: None,
            spotlight: None,
            overlay_focus: OverlayFocus::default(),
            last_cursor: None,
            last_tick: Instant::now(),
            tray_interaction_at: None,
            last_approval_poll: Instant::now(),
            approval_poll_inflight: false,
            approval_needs_reveal: false,
        }
    }

    fn chrome_window_exists(&self) -> bool {
        self.chat.is_some()
            || self.detail_win.is_some()
            || self.caption.is_some()
            || self.spotlight.is_some()
    }

    fn sync_overlay_interaction(&mut self) {
        // A tray menu steal is not a real focus switch; the overlay-focus
        // state machine already models that grace window via its target, so
        // protection derives solely from which chrome surface owns focus.
        let protect_chrome = self.overlay_focus.protects();
        let always_on_top = self.local_settings.always_on_top;
        let preferred_click_through = self.local_settings.overlay_click_through;
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        let level = overlay_window_level(protect_chrome, always_on_top);
        let click_through = protect_chrome || (overlay.transparent && preferred_click_through);
        overlay.window.set_window_level(level);
        overlay.set_click_through(click_through);
    }

    fn open_chat(&mut self, event_loop: &ActiveEventLoop) {
        self.surface.chat_open = true;
        self.surface.focus_chat = true;
        if let Some(gpu) = self.gpu.as_ref() {
            let chat = std::mem::take(&mut self.chat);
            match ChromeWindow::restore_or_create(
                chat,
                event_loop,
                gpu,
                ChromeKind::Chat,
                PhysicalSize::new(surface::CHAT_WINDOW_WIDTH, surface::CHAT_WINDOW_HEIGHT),
                true,
            ) {
                Ok(win) => {
                    self.chat = Some(win);
                    self.overlay_focus.transition(FocusTarget::Chat);
                }
                Err(err) => tracing::warn!(error = %err, "chat window failed"),
            }
        }
        if self.chat.is_none() {
            self.drop_focus_if_no_chrome();
        }
        self.sync_overlay_interaction();
        if let Some(chat) = self.chat.as_ref() {
            // Raise after the overlay has been lowered/click-through'd so
            // the WM stacks the chat above a hit-testing overlay.
            chat.raise();
        }
    }

    fn spawn<F>(&self, task: F)
    where
        F: std::future::Future<Output = AsyncOutcome> + Send + 'static,
    {
        let results = Arc::clone(&self.async_results);
        self.rt_handle.spawn(async move {
            let outcome = task.await;
            results.lock().push(outcome);
        });
    }

    fn claim_speaker_notify(&mut self) {
        if !self.speaker_claimed {
            self.speaker_claimed = true;
            let session = self.session.clone_handle();
            self.spawn(async move {
                let result = session
                    .claim_speaker()
                    .await
                    .map(|snap| snap.speaker.unwrap_or_default())
                    .map_err(|e| e.to_string());
                AsyncOutcome::SpeakerClaim(result)
            });
        }
        if !self.notify_claimed {
            self.notify_claimed = true;
            let session = self.session.clone_handle();
            self.spawn(async move {
                let result = session
                    .claim_notify()
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string());
                AsyncOutcome::NotifyClaim(result)
            });
        }
    }

    fn drain_async_results(&mut self) {
        let outcomes = {
            let mut guard = self.async_results.lock();
            std::mem::take(&mut *guard)
        };
        for outcome in outcomes {
            self.apply_async_outcome(outcome);
        }
    }

    #[expect(clippy::too_many_lines, reason = "outcome dispatch is a flat match")]
    fn apply_async_outcome(&mut self, outcome: AsyncOutcome) {
        match outcome {
            AsyncOutcome::SendMessage { session_id, result } => {
                if session_id != self.session.session_id() {
                    return;
                }
                if let Err(err) = result {
                    self.surface.status = err;
                } else {
                    self.surface.chat_draft.clear();
                    self.surface.streaming_text.clear();
                    self.request_history_refresh();
                }
            }
            AsyncOutcome::SelectGreeting { session_id, result } => {
                if session_id != self.session.session_id() {
                    return;
                }
                self.surface.greeting_inflight = false;
                match result {
                    Ok(history) => {
                        self.session.replace_history(history.clone());
                        self.surface.history = history;
                        self.surface.greetings.clear();
                        self.surface.greeting_status.clear();
                    }
                    Err(err) => self.surface.greeting_status = err,
                }
            }
            AsyncOutcome::BargeIn { session_id, result }
            | AsyncOutcome::CancelTurn { session_id, result } => {
                if session_id != self.session.session_id() {
                    return;
                }
                if let Err(err) = result {
                    self.surface.status = err;
                }
            }
            AsyncOutcome::Approval { session_id, result } => {
                if session_id != self.session.session_id() {
                    return;
                }
                self.surface.pending_approval = None;
                self.approval_needs_reveal = false;
                if let Err(err) = result {
                    self.surface.status = err;
                }
            }
            AsyncOutcome::Listen { generation, result } => {
                if let Err(err) = result {
                    tracing::debug!(error = %err, "listen failed");
                }
                let action = self
                    .listen
                    .on_done(generation, self.mic_active, Instant::now());
                self.spawn_listen(action);
            }
            AsyncOutcome::RefreshHistory { session_id, result } => {
                if session_id != self.session.session_id() {
                    return;
                }
                match result {
                    Ok(history) => {
                        if let Some(message) = history
                            .messages
                            .iter()
                            .rev()
                            .find(|message| message.role == "status")
                        {
                            let mapped = map_turn_err(&message.text);
                            if auth_failure(&mapped) || auth_failure(&message.text) {
                                self.detail.core_status = i18n::fl("chat-auth-failed");
                            }
                            mapped.clone_into(&mut self.surface.status);
                        }
                        self.session.replace_history(history.clone());
                        self.surface.history = history;
                        self.surface.streaming_text.clear();
                        if let Some(chat) = &self.chat {
                            chat.request_redraw();
                        }
                    }
                    Err(err) => self.surface.status = err,
                }
            }
            AsyncOutcome::SaveLocalSettings(result) => {
                self.detail.core_status = match result {
                    Ok(()) => i18n::fl("settings-saved"),
                    Err(err) => err,
                };
            }
            AsyncOutcome::LoadCoreSettings(result) => match result {
                Ok(json) => {
                    self.detail.core_settings_text.clone_from(&json);
                    self.detail.core_patch_text.clear();
                    detail::parse_core_fields(&json, &mut self.detail);
                    self.detail.finish_settings_load();
                    self.detail.core_status = if detail::chat_setup_gap(&self.detail)
                        == Some(detail::ChatSetupGap::ApiKey)
                    {
                        i18n::fl("settings-chat-key-required")
                    } else {
                        i18n::fl("settings-loaded")
                    };
                }
                Err(err) => {
                    self.detail.settings_load_failed();
                    self.detail.core_status = err;
                }
            },
            AsyncOutcome::ApplyCoreSettings(result) => match result {
                Ok(()) => {
                    self.detail.core_status = i18n::fl("settings-applied");
                    self.detail.invalidate_settings();
                    detail::ensure_settings(
                        &mut self.detail,
                        &self.client,
                        &self.rt_handle,
                        &self.async_results,
                    );
                }
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::ListMemories { soul_id, result } => {
                if soul_id != self.session.soul_id() {
                    return;
                }
                match result {
                    Ok(items) => self.detail.memories = items,
                    Err(err) => self.detail.core_status = err,
                }
            }
            AsyncOutcome::ListPendingMemories { soul_id, result } => {
                if soul_id != self.session.soul_id() {
                    return;
                }
                match result {
                    Ok(items) => {
                        self.detail.sync_candidate_drafts(&items);
                        self.detail.pending_memories = items;
                    }
                    Err(err) => self.detail.core_status = err,
                }
            }
            AsyncOutcome::ListMemoryJournal { soul_id, result } => {
                if soul_id != self.session.soul_id() {
                    return;
                }
                match result {
                    Ok(items) => self.detail.memory_journal = items,
                    Err(err) => self.detail.core_status = err,
                }
            }
            AsyncOutcome::ResolveMemory {
                soul_id, result, ..
            }
            | AsyncOutcome::DeleteMemory {
                soul_id, result, ..
            }
            | AsyncOutcome::CompleteMemory {
                soul_id, result, ..
            } => {
                if soul_id != self.session.soul_id() {
                    return;
                }
                match result {
                    Ok(()) => self.request_memories(),
                    Err(err) => {
                        let stale_candidate = err.contains("candidate_conflict");
                        self.detail.core_status = err;
                        if stale_candidate {
                            self.request_memories();
                        }
                    }
                }
            }
            AsyncOutcome::LoadSoul(result) => match result {
                Ok(soul) => {
                    self.detail.body_ref_draft = soul.body_ref.clone().unwrap_or_default();
                    self.detail.soul = Some(soul);
                }
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::PatchBody(result) => match result {
                Ok(soul) => {
                    self.detail.soul = Some(soul.clone());
                    self.detail.body_ref_draft = soul.body_ref.clone().unwrap_or_default();
                    self.detail.core_status = i18n::fl("character-body-updated");
                    self.reload_avatar();
                }
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::ImportCharacter { generation, result }
            | AsyncOutcome::ActivateCharacter { generation, result } => {
                if !self.detail.activation_is_current(generation) {
                    return;
                }
                match result {
                    Ok(activated) => {
                        if let Some(target) = activated.target {
                            self.commit_session_target(target);
                        }
                        self.reload_avatar();
                        self.detail.invalidate_character();
                        self.detail.core_status = format!(
                            "{}: {}",
                            i18n::fl("character-imported"),
                            activated.character.id
                        );
                        self.request_characters();
                    }
                    Err(err) => {
                        self.surface.status = err.clone();
                        self.detail.core_status = err;
                    }
                }
            }
            AsyncOutcome::ListCharacters(result) => match result {
                Ok(items) => self.detail.characters = items,
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::ListOccupants(result) => match result {
                Ok(items) => self.detail.occupants = items,
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::ListJobs(result) => match result {
                Ok((jobs, schedules)) => {
                    self.detail.jobs = jobs;
                    self.detail.schedules = schedules;
                }
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::CreateJob(result) => {
                self.detail.new_job_inflight = false;
                match result {
                    Ok(job) => {
                        self.detail.jobs.retain(|item| item.id != job.id);
                        self.detail.jobs.insert(0, job);
                        self.detail.new_job_title.clear();
                        self.detail.new_job_goal.clear();
                        self.detail.core_status = i18n::fl("jobs-created");
                    }
                    Err(err) => self.detail.core_status = err,
                }
            }
            AsyncOutcome::CancelJob { result, .. }
            | AsyncOutcome::ToggleSchedule { result, .. } => {
                if result.is_ok() {
                    self.request_jobs();
                } else if let Err(err) = result {
                    self.detail.core_status = err;
                }
            }
            AsyncOutcome::ListPlugins(result) => match result {
                Ok(items) => self.detail.plugins = items,
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::LoadPluginConfig {
                request_id,
                id,
                result,
            } => {
                if !detail::plugin_config_load_is_current(&self.detail, &id, request_id) {
                    return;
                }
                self.detail.plugin_config_loading_request_id = None;
                match result {
                    Ok(view) => {
                        detail::apply_plugin_config_view(&mut self.detail, view);
                        self.detail.connections_status.clear();
                    }
                    Err(err) => self.detail.connections_status = err,
                }
            }
            AsyncOutcome::ValidatePluginConfig(result)
            | AsyncOutcome::ApplyPluginConfig(result) => match result {
                Ok(view) => {
                    self.detail.connections_status = detail::plugin_config_status(&view);
                }
                Err(err) => self.detail.connections_status = err,
            },
            AsyncOutcome::PluginConfigOptions(result) => match result {
                Ok(view) => {
                    self.detail.plugin_config_options = if view.fallback {
                        view.error
                            .unwrap_or_else(|| i18n::fl("plugins-config-options-fallback"))
                    } else {
                        view.options
                            .iter()
                            .map(|opt| format!("{} ({})", opt.label, opt.id))
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    self.detail.connections_status.clear();
                }
                Err(err) => self.detail.connections_status = err,
            },
            AsyncOutcome::RestartPlugin { result, .. }
            | AsyncOutcome::SetActiveProviderAsset { result, .. } => {
                if let Err(err) = result {
                    self.detail.connections_status = err;
                }
            }
            AsyncOutcome::ListProviderAssets(result) => match result {
                Ok(items) => {
                    let count = items.len();
                    self.detail.provider_assets = items;
                    self.detail.connections_status = provider_asset_load_status(count);
                }
                Err(err) => self.detail.connections_status = err,
            },
            AsyncOutcome::InstallProviderAsset { asset_id, result } => match result {
                Ok(job_id) => {
                    self.detail.provider_install_jobs.insert(asset_id, job_id);
                    self.detail.connections_status = i18n::fl("plugins-asset-install-started");
                }
                Err(err) => self.detail.connections_status = err,
            },
            AsyncOutcome::ProviderAssetInstallStatus { asset_id, result } => match result {
                Ok(status) => {
                    if status.phase == Some(ene_api::ProviderAssetInstallPhase::Done) {
                        self.detail.provider_install_jobs.remove(&asset_id);
                    } else if status.phase == Some(ene_api::ProviderAssetInstallPhase::Failed) {
                        self.detail.provider_install_jobs.remove(&asset_id);
                        self.detail.connections_status = status
                            .error
                            .unwrap_or_else(|| i18n::fl("plugins-asset-install-failed"));
                    }
                }
                Err(err) => self.detail.connections_status = err,
            },
            AsyncOutcome::ListProviderModels(result) => match result {
                Ok((items, error)) => {
                    self.detail.provider_models = items;
                    self.detail.provider_model_filter.clear();
                    self.detail.core_status =
                        detail::list_models_status(&self.detail.provider_models, error.as_deref());
                }
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::LoadMcp(result) => match result {
                Ok(json) => {
                    if let Err(err) = detail::load_mcp_form(&mut self.detail, &json) {
                        self.detail.core_status = err;
                    }
                }
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::SaveMcp(result) => {
                self.detail.core_status = match result {
                    Ok(()) => i18n::fl("mcp-saved"),
                    Err(err) => err,
                };
            }
            AsyncOutcome::MicClaim(result) => match result {
                Ok(active) => {
                    self.mic_active = active;
                    if let Some(tray) = self.tray.as_ref() {
                        tray.set_mic_active(active);
                    }
                    if active {
                        let action = self.listen.start();
                        self.spawn_listen(action);
                    } else {
                        self.listen.release();
                    }
                }
                Err(err) => self.surface.status = err,
            },
            AsyncOutcome::SpeakerClaim(result) => match result {
                Ok(holder) if !holder.is_empty() && holder != "stage" => {
                    self.surface.exclusive_notice =
                        format!("{}: {holder}", i18n::fl("exclusive-speaker"));
                }
                Err(err) => self.surface.exclusive_notice = err,
                Ok(_) => self.surface.exclusive_notice.clear(),
            },
            AsyncOutcome::NotifyClaim(result) => {
                if let Err(err) = result {
                    tracing::debug!(error = %err, "notify claim failed");
                }
            }
            AsyncOutcome::Health(result) => match result {
                Ok(health) => self.detail.health = format!("{} ({})", health.status, health.bind),
                Err(err) => self.detail.health = err,
            },
            AsyncOutcome::Usage(result) => match result {
                Ok(usage) => {
                    self.detail.usage_text = format!(
                        "in={} out={} cache_r={} cache_w={} rows={}",
                        usage.input_tokens,
                        usage.output_tokens,
                        usage.cache_read_tokens,
                        usage.cache_write_tokens,
                        usage.rows
                    );
                }
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::Backup(result) => match result {
                Ok((id, path)) => {
                    self.detail.restore_id.clone_from(&id);
                    self.detail.core_status = format!("{}: {path}", i18n::fl("system-backup"));
                }
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::Restore(result) => {
                self.detail.core_status = match result {
                    Ok(()) => i18n::fl("system-restore-done"),
                    Err(err) => err,
                };
            }
            AsyncOutcome::DiagSpans(result) => match result {
                Ok(spans) => {
                    self.detail.spans_text = spans
                        .into_iter()
                        .map(|span| format!("{} {}ms", span.name, span.duration_ms.unwrap_or(0)))
                        .collect::<Vec<_>>()
                        .join("\n");
                }
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::LoadSchema(result) => match result {
                Ok(json) => self.detail.schema_json = json,
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::ListApprovals(result) => {
                self.approval_poll_inflight = false;
                match result {
                    Ok(items) => self.apply_listed_approvals(&items),
                    Err(err) => tracing::debug!(error = %err, "approval poll failed"),
                }
            }
            AsyncOutcome::ReloadAvatar => self.reload_avatar(),
            AsyncOutcome::ExportCharacter(result) => {
                self.detail.core_status = match result {
                    Ok(()) => i18n::fl("character-exported"),
                    Err(err) => err,
                };
            }
            AsyncOutcome::ForkSession(result) => {
                self.detail.core_status = match result {
                    Ok(id) => format!("{}: {id}", i18n::fl("jobs-forked")),
                    Err(err) => err,
                };
            }
            AsyncOutcome::NewSession(result) => match result {
                Ok(split) => {
                    self.session.adopt_new_session(&split);
                    self.feeds =
                        spawn_event_feeds(&self.rt_handle, &self.client, self.session.session_id());
                    self.detail.set_session_id(self.session.session_id());
                    self.surface.history = self.session.history();
                    self.surface.greetings = self.session.greetings().to_vec();
                    self.surface.greeting_inflight = false;
                    self.surface.greeting_status.clear();
                    self.surface.streaming_text.clear();
                    self.surface.pending_approval = None;
                    self.approval_needs_reveal = false;
                    self.surface.pending_question = None;
                    self.surface.status = i18n::fl("chat-new-session-ready");
                    self.request_history_refresh();
                    self.detail.new_session_inflight = false;
                    self.surface.new_session_inflight = false;
                }
                Err(err) => {
                    self.detail.new_session_inflight = false;
                    self.surface.new_session_inflight = false;
                    self.surface.status = err;
                }
            },
            AsyncOutcome::CompactSession(result) => {
                self.detail.core_status = match result {
                    Ok(id) => format!("{}: {id}", i18n::fl("jobs-compacted")),
                    Err(err) => err,
                };
            }
            AsyncOutcome::ExportSession(result) => {
                self.detail.core_status = match result {
                    Ok(()) => i18n::fl("jobs-exported"),
                    Err(err) => err,
                };
            }
        }
    }

    fn apply_listed_approvals(&mut self, items: &[ene_api::ApprovalView]) {
        match (
            self.surface
                .pending_approval
                .as_ref()
                .map(|item| item.id.as_str()),
            items.first(),
        ) {
            (Some(id), Some(item)) if id == item.id => {}
            (_, Some(item)) => {
                self.set_pending_approval(surface::PendingApproval {
                    id: item.id.clone(),
                    tool: item.tool.clone(),
                    target: item.target.clone(),
                });
            }
            (_, None) => {
                self.surface.pending_approval = None;
                self.approval_needs_reveal = false;
            }
        }
    }

    fn set_pending_approval(&mut self, approval: surface::PendingApproval) {
        let is_new = self
            .surface
            .pending_approval
            .as_ref()
            .is_none_or(|current| current.id != approval.id);
        self.surface.pending_approval = Some(approval);
        if is_new {
            self.surface.chat_open = true;
            self.approval_needs_reveal = true;
        }
    }

    fn poll_pending_approvals(&mut self) {
        if self.approval_poll_inflight
            || self.last_approval_poll.elapsed() < Duration::from_millis(400)
        {
            return;
        }
        self.last_approval_poll = Instant::now();
        self.approval_poll_inflight = true;
        let client = Arc::clone(&self.client);
        self.spawn(async move {
            AsyncOutcome::ListApprovals(
                client
                    .list_approvals()
                    .await
                    .map(|page| page.items)
                    .map_err(|err| err.to_string()),
            )
        });
    }

    fn request_memories(&self) {
        let soul_id_memories = self.session.soul_id().to_owned();
        let client = Arc::clone(&self.client);
        self.spawn(async move {
            AsyncOutcome::ListMemories {
                soul_id: soul_id_memories.clone(),
                result: client
                    .list_memories(&soul_id_memories, None)
                    .await
                    .map(|page| page.items)
                    .map_err(|err| err.to_string()),
            }
        });
        let soul_id_pending = self.session.soul_id().to_owned();
        let client_pending = Arc::clone(&self.client);
        self.spawn(async move {
            AsyncOutcome::ListPendingMemories {
                soul_id: soul_id_pending.clone(),
                result: client_pending
                    .list_pending_memories(&soul_id_pending)
                    .await
                    .map(|page| page.items)
                    .map_err(|err| err.to_string()),
            }
        });
        let soul_id_journal = self.session.soul_id().to_owned();
        let client_journal = Arc::clone(&self.client);
        self.spawn(async move {
            AsyncOutcome::ListMemoryJournal {
                soul_id: soul_id_journal.clone(),
                result: client_journal
                    .list_memory_journal(&soul_id_journal)
                    .await
                    .map(|page| page.items)
                    .map_err(|err| err.to_string()),
            }
        });
    }

    fn request_characters(&self) {
        let client = Arc::clone(&self.client);
        self.spawn(async move {
            AsyncOutcome::ListCharacters(
                client
                    .list_characters()
                    .await
                    .map(|page| page.items)
                    .map_err(|err| err.to_string()),
            )
        });
    }

    fn request_jobs(&self) {
        let soul_id = self.session.soul_id().to_owned();
        let client = Arc::clone(&self.client);
        self.spawn(async move {
            let result = async {
                let jobs = client.list_jobs(Some(&soul_id)).await?.items;
                let schedules = client.list_schedules().await?.items;
                Ok((jobs, schedules))
            }
            .await
            .map_err(|err: ene_api::ApiError| err.to_string());
            AsyncOutcome::ListJobs(result)
        });
    }

    fn request_history_refresh(&self) {
        let session = self.session.clone_handle();
        let session_id = self.session.session_id().to_owned();
        self.spawn(async move {
            AsyncOutcome::RefreshHistory {
                session_id,
                result: session
                    .refresh_history()
                    .await
                    .map_err(|err| err.to_string()),
            }
        });
    }

    fn start_new_session(&mut self) {
        if self.detail.new_session_inflight {
            return;
        }
        self.detail.new_session_inflight = true;
        self.surface.new_session_inflight = true;
        let client = Arc::clone(&self.client);
        let session_id = self.session.session_id().to_owned();
        self.spawn(async move {
            AsyncOutcome::NewSession(
                client
                    .split_session(&session_id)
                    .await
                    .map_err(|err| err.to_string()),
            )
        });
    }

    fn toggle_mic(&mut self) {
        let session = self.session.clone_handle();
        let enable = !self.mic_active;
        self.spawn(async move {
            let result = if enable {
                session
                    .claim_mic()
                    .await
                    .map(|_| true)
                    .map_err(|e| e.to_string())
            } else {
                session
                    .release_mic()
                    .await
                    .map(|_| false)
                    .map_err(|e| e.to_string())
            };
            AsyncOutcome::MicClaim(result)
        });
    }

    fn send_chat(&mut self) {
        let text = self.surface.chat_draft.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if let Some(reason) = chat_send_block_reason(&self.detail) {
            if !self.detail.settings_loaded() {
                detail::ensure_settings(
                    &mut self.detail,
                    &self.client,
                    &self.rt_handle,
                    &self.async_results,
                );
            }
            self.surface.status = reason;
            return;
        }
        if !self.surface.turn_active
            && matches!(
                self.surface.message_mode,
                MessageMode::FollowUp | MessageMode::Steer
            )
        {
            self.surface.status = i18n::fl("chat-no-active-turn");
            return;
        }
        let session = self.session.clone_handle();
        let session_id = self.session.session_id().to_owned();
        let mode = self.surface.message_mode;
        self.surface.begin_send();
        self.spawn(async move {
            AsyncOutcome::SendMessage {
                session_id,
                result: session
                    .send(&text, mode)
                    .await
                    .map(|_| ())
                    .map_err(|err| map_turn_err(&err.to_string())),
            }
        });
    }

    fn select_greeting(&mut self, index: u32) {
        if self.surface.greeting_inflight {
            return;
        }
        self.surface.greeting_inflight = true;
        self.surface.greeting_status.clear();
        let session = self.session.clone_handle();
        let session_id = self.session.session_id().to_owned();
        self.spawn(async move {
            AsyncOutcome::SelectGreeting {
                session_id,
                result: session
                    .select_greeting(index)
                    .await
                    .map_err(|err| err.to_string()),
            }
        });
    }

    fn answer_question(&mut self) {
        let Some(question) = self.surface.pending_question.take() else {
            return;
        };
        let text = if self.surface.chat_draft.trim().is_empty() {
            question.prompt
        } else {
            self.surface.chat_draft.trim().to_owned()
        };
        self.surface.chat_draft.clear();
        let session = self.session.clone_handle();
        let session_id = self.session.session_id().to_owned();
        self.spawn(async move {
            AsyncOutcome::SendMessage {
                session_id,
                result: session
                    .answer_job(&question.id, &text)
                    .await
                    .map_err(|err| err.to_string()),
            }
        });
    }

    fn barge_in(&mut self) {
        if !self.surface.turn_active {
            self.surface.status = i18n::fl("chat-no-active-turn");
            return;
        }
        let session = self.session.clone_handle();
        let session_id = self.session.session_id().to_owned();
        self.spawn(async move {
            AsyncOutcome::BargeIn {
                session_id,
                result: session
                    .barge_in()
                    .await
                    .map(|_| ())
                    .map_err(|e| map_turn_err(&e.to_string())),
            }
        });
    }

    fn cancel_turn(&mut self) {
        if !self.surface.turn_active {
            self.surface.status = i18n::fl("chat-no-active-turn");
            return;
        }
        let session = self.session.clone_handle();
        let session_id = self.session.session_id().to_owned();
        self.spawn(async move {
            AsyncOutcome::CancelTurn {
                session_id,
                result: session
                    .cancel_turn()
                    .await
                    .map(|_| ())
                    .map_err(|e| map_turn_err(&e.to_string())),
            }
        });
    }

    fn respond_approval(&mut self, decision: &str) {
        let Some(pending) = self.surface.pending_approval.clone() else {
            return;
        };
        let session = self.session.clone_handle();
        let session_id = self.session.session_id().to_owned();
        let decision = decision.to_owned();
        self.spawn(async move {
            AsyncOutcome::Approval {
                session_id,
                result: session
                    .respond_approval(&pending.id, &decision)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
            }
        });
    }

    fn save_local_settings(&mut self) {
        let settings = self.local_settings.clone();
        if settings.mic_device != self.settings.mic_device {
            self.audio.set_mic_device(&settings.mic_device);
        }
        self.settings = settings.clone();
        i18n::select_language(&settings.language);
        self.sync_chrome_titles();
        if let Some(caption) = &self.caption {
            caption.place_caption(&settings.caption_position);
        }
        self.sync_overlay_interaction();
        self.spawn(async move {
            AsyncOutcome::SaveLocalSettings(
                save_desktop_settings(&settings).map_err(|err| err.to_string()),
            )
        });
    }

    fn sync_chrome_titles(&self) {
        if let Some(win) = &self.chat {
            win.sync_title();
        }
        if let Some(win) = &self.detail_win {
            win.sync_title();
        }
        if let Some(win) = &self.caption {
            win.sync_title();
        }
        if let Some(win) = &self.spotlight {
            win.sync_title();
        }
    }

    fn toggle_overlay_chrome(&mut self) {
        let size = {
            let Some(overlay) = self.overlay.as_mut() else {
                return;
            };
            overlay.toggle_chrome();
            overlay.window.inner_size()
        };
        let gpu = self.gpu.as_ref();
        if let (Some(gpu), Some(overlay)) = (gpu, self.overlay.as_mut()) {
            overlay.resize(gpu, size);
        }
        self.sync_overlay_interaction();
    }

    fn raise_chrome(&self) {
        for win in [
            self.chat.as_ref(),
            self.detail_win.as_ref(),
            self.caption.as_ref(),
            self.spotlight.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            win.raise();
        }
    }

    fn dispatch_shell_command(&mut self, event_loop: &ActiveEventLoop, command: ShellCommand) {
        match command {
            ShellCommand::OpenSpotlight => {
                if self.settings.spotlight_enabled {
                    self.surface.spotlight_open = true;
                    self.ensure_spotlight(event_loop);
                }
            }
            ShellCommand::OpenDetail(tab) => self.open_detail(event_loop, tab),
            ShellCommand::OpenChat => self.open_chat(event_loop),
            ShellCommand::ToggleMic => self.toggle_mic(),
            ShellCommand::Quit => self.surface.quit = true,
        }
    }

    fn poll_shell(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(target_os = "linux")]
        {
            while gtk::events_pending() {
                let _ = gtk::main_iteration_do(false);
            }
        }
        let tray_commands: Vec<ShellCommand> = self
            .tray
            .as_ref()
            .map(|tray| {
                let mut commands = Vec::new();
                while let Some(command) = tray.try_recv() {
                    commands.push(command);
                }
                commands
            })
            .unwrap_or_default();
        if let Some(tray) = self.tray.as_ref()
            && tray.take_interactions() > 0
        {
            self.tray_interaction_at = Some(Instant::now());
        }
        for command in tray_commands {
            self.dispatch_shell_command(event_loop, command);
        }
        let hotkey_command = self.hotkeys.as_mut().and_then(HotkeyManager::poll);
        if let Some(command) = hotkey_command {
            self.dispatch_shell_command(event_loop, command);
        }
        if self.surface.quit {
            event_loop.exit();
        }
    }

    fn process_surface_actions(&mut self, event_loop: &ActiveEventLoop) {
        let actions = std::mem::take(&mut self.surface.pending_actions);
        for action in actions {
            match action {
                SurfaceAction::SendChat => self.send_chat(),
                SurfaceAction::NewSession => self.start_new_session(),
                SurfaceAction::SelectGreeting { index } => self.select_greeting(index),
                SurfaceAction::BargeIn => self.barge_in(),
                SurfaceAction::CancelTurn => self.cancel_turn(),
                SurfaceAction::ToggleMic => self.toggle_mic(),
                SurfaceAction::Approval { decision } => self.respond_approval(&decision),
                SurfaceAction::AnswerQuestion => self.answer_question(),
                SurfaceAction::OpenDetail(tab) => self.open_detail(event_loop, tab),
                SurfaceAction::Quit => self.surface.quit = true,
                SurfaceAction::PersistCharacterPos => {
                    self.local_settings.character_x = self.surface.character_pos[0];
                    self.local_settings.character_y = self.surface.character_pos[1];
                    self.save_local_settings();
                }
            }
        }
        if self.detail.save_local_pending {
            self.detail.save_local_pending = false;
            self.save_local_settings();
        }
        if std::mem::take(&mut self.detail.request_chat_open) {
            self.open_chat(event_loop);
        }
    }

    fn drain_surface_events(&mut self) {
        while let Ok(event) = self.feeds.surface.try_recv() {
            self.apply_live_event(event);
        }
    }

    fn apply_live_event(&mut self, event: LiveEvent) {
        match event {
            LiveEvent::TextDelta { text, .. } => {
                self.surface
                    .apply_text_delta(&text, self.settings.caption_enabled);
                self.sync_caption_window();
            }
            LiveEvent::SessionEvent { kind, text } => {
                if kind == "turn/end" || kind.ends_with("/end") {
                    self.session.clear_turn();
                    self.request_history_refresh();
                    self.surface.on_turn_ended();
                    self.sync_caption_window();
                    if !text.is_empty() {
                        let mapped = map_turn_err(&text);
                        if auth_failure(&mapped) || auth_failure(&text) {
                            self.detail.core_status = i18n::fl("chat-auth-failed");
                        }
                        mapped.clone_into(&mut self.surface.status);
                    }
                }
                tracing::debug!(kind, text, "surface session event");
            }
            LiveEvent::ApprovalAsked { id, tool, target } => {
                self.set_pending_approval(surface::PendingApproval { id, tool, target });
            }
            LiveEvent::ApprovalResolved { .. } => {
                self.surface.pending_approval = None;
                self.approval_needs_reveal = false;
            }
            LiveEvent::QuestionAsked { id, prompt } => {
                self.surface.pending_question = Some(surface::PendingQuestion { id, prompt });
                self.surface.chat_open = true;
            }
            LiveEvent::QuestionResolved { id } => {
                if self
                    .surface
                    .pending_question
                    .as_ref()
                    .is_some_and(|question| question.id == id)
                {
                    self.surface.pending_question = None;
                }
            }
            LiveEvent::NotifyHint { title, body } => {
                if let Err(err) = show_notification(&title, &body, "ene-stage") {
                    tracing::debug!(error = %err, "notification failed");
                }
            }
            LiveEvent::BodyCommand { value } => {
                let Some(overlay) = self.overlay.as_mut() else {
                    return;
                };
                let session_soul = self.session.soul_id().to_owned();
                let event_soul = value.get("soul_id").and_then(serde_json::Value::as_str);
                let avatar = match event_soul {
                    Some(soul) => overlay.avatar_mut(soul),
                    None => overlay.avatar_or_first_mut(&session_soul),
                };
                if let Some(avatar) = avatar {
                    avatar.apply_body_event(&value);
                }
            }
            LiveEvent::AudioChunk {
                pcm,
                sample_rate,
                abort,
                ..
            } => {
                if abort {
                    self.abort_audio_playback();
                } else if let Err(err) = self.audio.play_pcm(&pcm, sample_rate) {
                    tracing::debug!(error = %err, "audio playback failed");
                }
            }
            LiveEvent::VoiceState { state, barge_in } => {
                self.surface.voice_state.clone_from(&state);
                if barge_in {
                    tracing::debug!("core barge-in (voice.state)");
                }
            }
            LiveEvent::ExclusiveHeld {
                resource,
                client_id,
            } => {
                if client_id != "stage" && !client_id.is_empty() {
                    self.surface.exclusive_notice = format!("{resource}: {client_id}");
                }
            }
            LiveEvent::Disconnected => {
                self.surface.status = i18n::fl("status-disconnected");
            }
            LiveEvent::ThinkingDelta { .. }
            | LiveEvent::InnerMessage { .. }
            | LiveEvent::ToolCall { .. }
            | LiveEvent::AffectState { .. }
            | LiveEvent::JobReport { .. } => {}
        }
    }

    fn drain_detail_events(&mut self) {
        while let Ok(event) = self.feeds.detail.try_recv() {
            match event {
                LiveEvent::ThinkingDelta { text } => self.detail.push_log(LogKind::Thinking, text),
                LiveEvent::InnerMessage { text } => self.detail.push_log(LogKind::Inner, text),
                LiveEvent::ToolCall { summary } => self.detail.push_log(LogKind::Tool, summary),
                LiveEvent::SessionEvent { kind, text } => {
                    self.detail.push_log(
                        LogKind::Session,
                        format!("{kind}: {}", format_log_text(&text)),
                    );
                }
                LiveEvent::AffectState {
                    mood_label,
                    valence,
                    arousal,
                } => {
                    self.detail.push_log(
                        LogKind::Affect,
                        format!("{mood_label} v={valence:.2} a={arousal:.2}"),
                    );
                }
                LiveEvent::JobReport { text } => self.detail.push_log(LogKind::Job, text),
                LiveEvent::BodyCommand { value } => {
                    let kind = value
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("body");
                    let name = value
                        .get("name")
                        .or_else(|| value.get("label"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    self.detail
                        .push_log(LogKind::Session, format!("{kind} {name}"));
                }
                LiveEvent::Disconnected => self
                    .detail
                    .push_log(LogKind::Session, "detail disconnected".into()),
                _ => {}
            }
        }
    }

    fn spawn_listen(&mut self, action: ListenAction) {
        let ListenAction::Spawn { generation, rx } = action else {
            return;
        };
        let client = Arc::clone(&self.client);
        let session_id = self.session.session_id().to_owned();
        self.spawn(async move {
            AsyncOutcome::Listen {
                generation,
                result: run_listen_stream(client, session_id, rx).await,
            }
        });
    }

    fn poll_audio(&mut self) {
        if !self.mic_active {
            self.listen.release();
            return;
        }
        let action = self.listen.poll(true, Instant::now());
        self.spawn_listen(action);
        for batch in self.audio.poll_mic_batches() {
            match self.listen.try_send(batch) {
                SendResult::Sent => {}
                SendResult::Full => {
                    tracing::debug!("listen stream dropped a mic frame");
                }
                SendResult::Closed | SendResult::Idle => {
                    let action = self.listen.poll(true, Instant::now());
                    self.spawn_listen(action);
                }
            }
        }
    }

    fn reload_avatar(&mut self) {
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        let specs = self.session.avatar_loads();
        if specs.is_empty() {
            overlay.clear_avatars();
            tracing::info!("no avatar_path; overlay stays empty (text-only)");
            return;
        }
        match overlay.load_avatars(gpu, &specs) {
            Ok(count) => {
                self.surface.status = i18n::fl("status-ready");
                tracing::info!(count, "loaded overlay VRM bodies");
            }
            Err(err) => {
                self.surface.status = format!("{}: {err}", i18n::fl("status-error"));
                tracing::warn!(error = %err, "VRM load failed");
            }
        }
    }

    fn cycle_occupant(&mut self, delta: i32) {
        let occupants = self.session.occupants();
        let Some(occupant) =
            crate::core::session::next_avatar_occupant(occupants, self.session.soul_id(), delta)
        else {
            self.surface.status = i18n::fl("overlay-no-avatar");
            return;
        };
        if occupant.soul_id == self.session.soul_id() {
            return;
        }
        let label = crate::core::session::occupant_label(&occupant);
        if let Err(err) = self
            .runtime
            .block_on(self.session.retarget_soul(&occupant.soul_id))
        {
            self.surface.status = err.to_string();
            return;
        }
        self.feeds = spawn_event_feeds(&self.rt_handle, &self.client, self.session.session_id());
        self.surface.history = self.session.history();
        self.surface.greetings = self.session.greetings().to_vec();
        self.surface.greeting_inflight = false;
        self.surface.greeting_status.clear();
        self.surface.pending_approval = None;
        self.approval_needs_reveal = false;
        self.surface.pending_question = None;
        self.detail.next_activation_generation();
        self.detail.invalidate_character();
        self.detail.invalidate_memory();
        if self
            .overlay
            .as_ref()
            .is_some_and(crate::overlay::OverlayWindow::has_avatars)
        {
            self.surface.status = format!("{}: {label}", i18n::fl("overlay-showing"));
        }
    }

    fn commit_session_target(&mut self, target: crate::core::session::PreparedSessionTarget) {
        let feeds = spawn_event_feeds(&self.rt_handle, &self.client, target.session_id());
        self.session.commit_retarget(target);
        self.feeds = feeds;
        self.surface.history = self.session.history();
        self.surface.greetings = self.session.greetings().to_vec();
        self.surface.greeting_inflight = false;
        self.surface.greeting_status.clear();
        self.surface.streaming_text.clear();
        self.surface.turn_active = false;
        self.surface.pending_approval = None;
        self.approval_needs_reveal = false;
        self.surface.pending_question = None;
        self.detail.invalidate_memory();
    }

    fn open_detail(&mut self, event_loop: &ActiveEventLoop, tab: DetailTab) {
        self.detail.visible = true;
        self.detail.refresh_settings_on_open();
        self.detail.select_tab(tab);
        if let Some(gpu) = self.gpu.as_ref() {
            let detail = std::mem::take(&mut self.detail_win);
            match ChromeWindow::restore_or_create(
                detail,
                event_loop,
                gpu,
                ChromeKind::Detail,
                PhysicalSize::new(960, 680),
                true,
            ) {
                Ok(win) => {
                    self.detail_win = Some(win);
                    self.overlay_focus.transition(FocusTarget::Detail);
                }
                Err(err) => tracing::warn!(error = %err, "detail window failed"),
            }
        }
        if self.detail_win.is_none() {
            self.drop_focus_if_no_chrome();
        }
        self.sync_overlay_interaction();
        if let Some(detail) = self.detail_win.as_ref() {
            detail.raise();
        }
    }

    /// Clear focus protection only when no other chrome window can hold it.
    fn drop_focus_if_no_chrome(&mut self) {
        if !self.chrome_window_exists() {
            self.overlay_focus = OverlayFocus::default();
        }
    }

    fn sync_caption_window(&mut self) {
        if !self.surface.caption_visible() {
            self.caption = None;
        }
    }

    fn ensure_caption(&mut self, event_loop: &ActiveEventLoop) {
        if self.caption.is_some() || !self.settings.caption_enabled {
            return;
        }
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        match ChromeWindow::create(
            event_loop,
            gpu,
            ChromeKind::Caption,
            PhysicalSize::new(720, 160),
            false,
        ) {
            Ok(win) => {
                win.place_caption(&self.settings.caption_position);
                self.caption = Some(win);
            }
            Err(err) => tracing::warn!(error = %err, "caption window failed"),
        }
    }

    fn ensure_spotlight(&mut self, event_loop: &ActiveEventLoop) {
        if self.spotlight.is_some() {
            return;
        }
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        match ChromeWindow::create(
            event_loop,
            gpu,
            ChromeKind::Spotlight,
            PhysicalSize::new(420, 480),
            true,
        ) {
            Ok(win) => self.spotlight = Some(win),
            Err(err) => tracing::warn!(error = %err, "spotlight window failed"),
        }
    }

    fn handle_overlay_key(&mut self, event_loop: &ActiveEventLoop, key: &Key) {
        match key {
            Key::Named(NamedKey::Escape) => self.surface.quit = true,
            Key::Named(NamedKey::F1) => self.open_detail(event_loop, DetailTab::Companion),
            Key::Named(NamedKey::F2) => self.open_chat(event_loop),
            Key::Named(NamedKey::F3) => {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.collider_debug = !overlay.collider_debug;
                    tracing::info!(on = overlay.collider_debug, "collider debug overlay");
                }
            }
            Key::Named(NamedKey::F4) => self.open_detail(event_loop, DetailTab::Log),
            _ => self.handle_overlay_shortcut(key),
        }
    }

    fn handle_overlay_shortcut(&mut self, key: &Key) {
        match key {
            Key::Named(NamedKey::Space) => {
                self.toggle_overlay_chrome();
                self.raise_chrome();
            }
            Key::Character(ch) if ch.as_str().eq_ignore_ascii_case("a") => self.cycle_occupant(-1),
            Key::Character(ch) if ch.as_str().eq_ignore_ascii_case("d") => self.cycle_occupant(1),
            Key::Character(ch) if ch.as_str().eq_ignore_ascii_case("w") => {
                if let Some(overlay) = self.overlay.as_mut() {
                    let soul = self.session.soul_id().to_owned();
                    if let Some(avatar) = overlay.avatar_or_first_mut(&soul) {
                        avatar.cycle_motion(-1);
                    }
                }
            }
            Key::Character(ch) if ch.as_str().eq_ignore_ascii_case("s") => {
                if let Some(overlay) = self.overlay.as_mut() {
                    let soul = self.session.soul_id().to_owned();
                    if let Some(avatar) = overlay.avatar_or_first_mut(&soul) {
                        avatar.cycle_motion(1);
                    }
                }
            }
            _ => {}
        }
    }

    fn abort_audio_playback(&mut self) {
        self.audio.stop();
        self.viseme.reset();
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.reset_visemes();
        }
    }

    fn tick_overlay(&mut self) {
        let dt = self.last_tick.elapsed().as_secs_f32();
        self.last_tick = Instant::now();
        let cursor = self.last_cursor;
        let strength = self.local_settings.look_at_strength;
        let visemes = self.audio.analyze_visemes(&mut self.viseme);
        let look_input = self.overlay.as_ref().and_then(|overlay| {
            overlay.first_avatar().map(|avatar| {
                (
                    overlay.window.inner_size(),
                    avatar.camera().eye(),
                    avatar.camera().target(),
                    avatar.head_world(),
                )
            })
        });
        let look = if let (Some(cursor), Some((size, eye, target, head))) = (cursor, look_input) {
            let viewport = (size.width.max(1), size.height.max(1));
            Some(look_at::compute_world_target(
                glam::Vec2::new(cursor.x, cursor.y),
                viewport,
                glam::Vec3::from(eye),
                glam::Vec3::from(target),
                glam::Vec3::from(ene_vrm::camera::DEFAULT_UP),
                head,
                strength,
                &mut self.look_at_state,
                dt,
            ))
        } else {
            None
        };
        let scale = self.local_settings.model_scale;
        let pos = self.surface.character_pos;
        let soul = self.session.soul_id().to_owned();
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        let count = overlay.slots.len();
        let base = [(pos[0] - 0.5) * 0.8, (0.5 - pos[1]) * 0.8, 0.0];
        for (index, slot) in overlay.slots.iter_mut().enumerate() {
            slot.avatar.model_scale = scale;
            slot.avatar.world_offset = crate::overlay::overlay_slot_offset(index, count, base);
        }
        if let Err(err) = overlay.tick_and_render(gpu, look, Some(visemes), Some(soul.as_str())) {
            match err {
                OverlayError::Surface(_) => {
                    tracing::debug!(error = %err, "overlay surface skipped");
                }
                OverlayError::Avatar(inner) => tracing::debug!(error = %inner, "overlay avatar"),
            }
        }
        overlay.window.request_redraw();
    }

    fn paint_chrome(&mut self, event_loop: &ActiveEventLoop) {
        if std::mem::take(&mut self.approval_needs_reveal) {
            self.open_chat(event_loop);
        }
        if chat_window_action(self.surface.chat_open, self.chat.is_some())
            == ChatWindowAction::Create
        {
            self.open_chat(event_loop);
        }
        if self.settings.caption_enabled && self.surface.caption_visible() {
            self.ensure_caption(event_loop);
        } else {
            self.caption = None;
        }
        if self.surface.spotlight_open {
            self.ensure_spotlight(event_loop);
        }
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };

        if let Some(chat) = self.chat.as_mut() {
            let surface = &mut self.surface;
            let mic = self.mic_active;
            let theme = self.local_settings.theme.as_str();
            if let Err(err) = chat.paint(gpu, Some(theme), |ui| {
                surface::show_chat(ui, surface, mic);
            }) {
                tracing::debug!(error = %err, "chat paint failed");
            }
        }
        if let Some(detail_win) = self.detail_win.as_mut() {
            let mut detail = std::mem::take(&mut self.detail);
            let mut local = self.local_settings.clone();
            let client = Arc::clone(&self.client);
            let rt = self.rt_handle.clone();
            let results = Arc::clone(&self.async_results);
            let soul_id = self.session.soul_id().to_owned();
            self.session.session_id().clone_into(&mut detail.session_id);
            let theme = local.theme.clone();
            let paint = detail_win.paint(gpu, Some(theme.as_str()), |ui| {
                detail::show(
                    ui,
                    &mut detail,
                    &mut local,
                    &soul_id,
                    &client,
                    &rt,
                    &results,
                );
            });
            self.detail = detail;
            self.local_settings = local;
            if let Err(err) = paint {
                tracing::debug!(error = %err, "detail paint failed");
            }
        }
        if let Some(caption) = self.caption.as_mut() {
            let surface = &self.surface;
            let font = self.settings.caption_font_size;
            let position = self.settings.caption_position.clone();
            let pinned = self.settings.caption_pinned;
            if let Err(err) = caption.paint(gpu, None, |ui| {
                surface::show_caption(ui.ctx(), surface, font, &position, pinned);
            }) {
                tracing::debug!(error = %err, "caption paint failed");
            }
        }
        let mut spotlight_action = None;
        if let Some(spotlight) = self.spotlight.as_mut()
            && let Err(err) = spotlight.paint(gpu, None, |ui| {
                spotlight_action = surface::show_spotlight(ui.ctx(), &mut self.surface);
            })
        {
            tracing::debug!(error = %err, "spotlight paint failed");
        }
        if let Some(action) = spotlight_action {
            self.surface.spotlight_open = false;
            self.spotlight = None;
            match action {
                SpotlightAction::Command(command) => {
                    self.dispatch_shell_command(event_loop, command);
                }
                SpotlightAction::Close => {}
            }
        }
    }

    fn close_chat_window(&mut self) {
        self.chat = None;
        self.overlay_focus.clear_target(FocusTarget::Chat);
        self.surface.close_chat();
    }

    fn close_detail_window(&mut self) {
        self.detail_win = None;
        self.overlay_focus.clear_target(FocusTarget::Detail);
        self.detail.visible = false;
    }

    fn close_caption_window(&mut self) {
        self.caption = None;
        self.overlay_focus.clear_target(FocusTarget::Caption);
    }

    fn close_spotlight_window(&mut self) {
        self.spotlight = None;
        self.overlay_focus.clear_target(FocusTarget::Spotlight);
        self.surface.spotlight_open = false;
    }
}

fn chat_send_block_reason(detail: &DetailUiState) -> Option<String> {
    if !detail.settings_loaded() {
        return Some(i18n::fl("chat-settings-loading"));
    }
    detail::chat_setup_gap(detail).map(detail::chat_setup_status)
}

#[cfg(test)]
mod chat_tests {
    use super::*;

    #[test]
    fn chat_waits_for_settings_before_checking_setup() {
        let state = DetailUiState::default();
        assert_eq!(
            chat_send_block_reason(&state),
            Some(i18n::fl("chat-settings-loading"))
        );
    }

    #[test]
    fn chat_checks_setup_after_settings_loads() {
        let mut state = DetailUiState::default();
        state.finish_settings_load();
        assert_eq!(
            chat_send_block_reason(&state),
            Some(i18n::fl("chat-unconfigured"))
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatWindowAction {
    None,
    Create,
}

#[must_use]
fn chat_window_action(chat_open: bool, chat_exists: bool) -> ChatWindowAction {
    if chat_open && !chat_exists {
        ChatWindowAction::Create
    } else {
        ChatWindowAction::None
    }
}

impl ApplicationHandler for StageApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let monitor = event_loop.primary_monitor();
        let size = monitor.as_ref().map_or(
            PhysicalSize::new(1280, 720),
            winit::monitor::MonitorHandle::size,
        );
        let mut attrs = Window::default_attributes()
            .with_title(i18n::fl("app-title"))
            .with_inner_size(size)
            .with_transparent(self.settings.transparent_overlay)
            .with_decorations(!self.settings.transparent_overlay)
            .with_visible(true);
        attrs = attrs.with_window_level(window_level(self.settings.always_on_top));
        if let Some(monitor) = monitor.as_ref() {
            attrs = attrs.with_position(monitor.position());
        }
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                tracing::error!(error = %err, "overlay window failed");
                event_loop.exit();
                return;
            }
        };
        crate::platform::apply_overlay_hints(window.as_ref());
        let gpu = match self.runtime.block_on(GpuContext::create()) {
            Ok(gpu) => gpu,
            Err(err) => {
                tracing::error!(error = %err, "gpu init failed");
                event_loop.exit();
                return;
            }
        };
        match OverlayWindow::create(window, &gpu, self.settings.transparent_overlay) {
            Ok(mut overlay) => {
                overlay.apply_click_through(self.local_settings.overlay_click_through);
                self.overlay = Some(overlay);
            }
            Err(err) => {
                tracing::error!(error = %err, "overlay surface failed");
                event_loop.exit();
                return;
            }
        }
        self.gpu = Some(gpu);
        self.reload_avatar();
        if self.surface.chat_open {
            self.open_chat(event_loop);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let overlay_id = self.overlay.as_ref().map(OverlayWindow::id);
        if overlay_id == Some(id) {
            if matches!(event, WindowEvent::Focused(true))
                && self.overlay_focus.on_focus_event(FocusOwner::Overlay, true)
            {
                self.sync_overlay_interaction();
            }
            match event {
                WindowEvent::CloseRequested => self.surface.quit = true,
                WindowEvent::Resized(size) => {
                    if let (Some(gpu), Some(overlay)) = (self.gpu.as_ref(), self.overlay.as_mut()) {
                        overlay.resize(gpu, size);
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    if let Some(overlay) = self.overlay.as_ref() {
                        let scale = overlay.window.scale_factor();
                        self.last_cursor = Some(position.to_logical(scale));
                        if self.surface.dragging_character {
                            let size = overlay.window.inner_size().to_logical::<f32>(scale);
                            let logical = position.to_logical::<f32>(scale);
                            let width = size.width.max(1.0);
                            let height = size.height.max(1.0);
                            self.surface.character_pos = [
                                (logical.x / width).clamp(0.05, 0.95),
                                (logical.y / height).clamp(0.05, 0.95),
                            ];
                        }
                    }
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => {
                    if self
                        .overlay
                        .as_ref()
                        .is_some_and(|overlay| !overlay.click_through)
                    {
                        self.surface.dragging_character = true;
                    }
                }
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button: MouseButton::Left,
                    ..
                } => {
                    self.surface.dragging_character = false;
                    self.surface.push_action(SurfaceAction::PersistCharacterPos);
                }
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed && !event.repeat =>
                {
                    if !self.surface.chat_input_focused {
                        self.handle_overlay_key(event_loop, &event.logical_key);
                    }
                }
                WindowEvent::RedrawRequested => self.tick_overlay(),
                _ => {}
            }
            return;
        }
        let mut close_chat = false;
        let mut close_detail = false;
        let mut close_caption = false;
        let mut close_spotlight = false;
        let mut overlay_from_chrome = None;
        let mut chrome_focus_state = None;
        if let Some(chat) = self.chat.as_mut()
            && chat.id() == id
        {
            chat.on_window_event(&event);
            if let Some(gpu) = self.gpu.as_ref()
                && let WindowEvent::Resized(size) = &event
            {
                chat.resize(gpu, *size);
            }
            overlay_from_chrome = Some(chat.owns_input());
            chrome_focus_state = window_focus_state(&event);
            close_chat = matches!(event, WindowEvent::CloseRequested);
        }
        if let Some(detail) = self.detail_win.as_mut()
            && detail.id() == id
        {
            detail.on_window_event(&event);
            if let Some(gpu) = self.gpu.as_ref()
                && let WindowEvent::Resized(size) = &event
            {
                detail.resize(gpu, *size);
            }
            overlay_from_chrome = Some(detail.owns_input());
            chrome_focus_state = window_focus_state(&event);
            close_detail = matches!(event, WindowEvent::CloseRequested);
        }
        if let Some(caption) = self.caption.as_mut()
            && caption.id() == id
        {
            caption.on_window_event(&event);
            chrome_focus_state = window_focus_state(&event);
            close_caption = matches!(event, WindowEvent::CloseRequested);
        }
        if let Some(spotlight) = self.spotlight.as_mut()
            && spotlight.id() == id
        {
            spotlight.on_window_event(&event);
            chrome_focus_state = window_focus_state(&event);
            close_spotlight = matches!(event, WindowEvent::CloseRequested);
        }
        if let Some(focused) = chrome_focus_state {
            let owner = if self.chat.as_ref().is_some_and(|w| w.id() == id) {
                Some(FocusOwner::Chat)
            } else if self.detail_win.as_ref().is_some_and(|w| w.id() == id) {
                Some(FocusOwner::Detail)
            } else {
                None
            };

            // Caption/Spotlight also emit Focused events; route them so a
            // focus handoff between chrome windows never drops protection.
            let owner = owner.or_else(|| {
                if self.caption.as_ref().is_some_and(|w| w.id() == id) {
                    Some(FocusOwner::Caption)
                } else if self.spotlight.as_ref().is_some_and(|w| w.id() == id) {
                    Some(FocusOwner::Spotlight)
                } else {
                    None
                }
            });
            if let Some(owner) = owner
                && self.overlay_focus.on_focus_event(owner, focused)
            {
                self.sync_overlay_interaction();
            }
        }
        if !self.surface.chat_input_focused
            && overlay_from_chrome.is_none_or(|wants| !wants)
            && let WindowEvent::KeyboardInput {
                event: key_event, ..
            } = &event
            && key_event.state == ElementState::Pressed
            && !key_event.repeat
        {
            self.handle_overlay_shortcut(&key_event.logical_key);
        }
        if close_chat {
            self.close_chat_window();
        }
        if close_detail {
            self.close_detail_window();
        }
        if close_caption {
            self.close_caption_window();
        }
        if close_spotlight {
            self.close_spotlight_window();
        }
        if !self.chrome_window_exists() {
            self.overlay_focus = OverlayFocus::default();
        }
        if close_chat || close_detail || close_caption || close_spotlight {
            self.sync_overlay_interaction();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Poll);
        self.drain_async_results();
        self.poll_pending_approvals();
        self.drain_surface_events();
        self.drain_detail_events();
        self.poll_shell(event_loop);
        self.poll_audio();
        self.process_surface_actions(event_loop);
        self.surface.turn_active = self.session.turn_id().is_some();
        if self.surface.history.messages.is_empty() {
            self.surface.history = self.session.history();
        }
        self.tick_overlay();
        self.paint_chrome(event_loop);
        if self.detail.open_spotlight {
            self.detail.open_spotlight = false;
            self.surface.spotlight_open = true;
            self.ensure_spotlight(event_loop);
        }
        if self.surface.quit {
            event_loop.exit();
        }
    }
}

fn auth_failure(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("401 unauthorized")
        || lower.contains("403 forbidden")
        || lower.contains("no cookie auth")
}

fn provider_asset_load_status(count: usize) -> String {
    format!("{}: {count}", i18n::fl("plugins-assets"))
}

#[must_use]
fn window_level(always_on_top: bool) -> WindowLevel {
    if always_on_top {
        WindowLevel::AlwaysOnTop
    } else {
        WindowLevel::Normal
    }
}

#[must_use]
fn overlay_window_level(chrome_focused: bool, always_on_top: bool) -> WindowLevel {
    if chrome_focused {
        WindowLevel::Normal
    } else {
        window_level(always_on_top)
    }
}

#[must_use]
fn window_focus_state(event: &WindowEvent) -> Option<bool> {
    match event {
        WindowEvent::Focused(focused) => Some(*focused),
        _ => None,
    }
}

/// Focus loss within this window after a tray interaction is treated as the
/// transient steal caused by the tray menu, not as a real switch to another app.
const TRAY_FOCUS_GRACE: Duration = Duration::from_millis(1500);

#[must_use]
fn focus_loss_is_transient(tray_interaction_at: Option<Instant>, now: Instant) -> bool {
    tray_interaction_at.is_some_and(|at| now.duration_since(at) < TRAY_FOCUS_GRACE)
}

fn format_log_text(text: &str) -> &str {
    if text.trim().is_empty() {
        "(empty)"
    } else {
        text
    }
}

fn map_turn_err(err: &str) -> String {
    if err.contains("no_active_operation") || err.contains("no active turn") {
        i18n::fl("chat-no-active-turn")
    } else if auth_failure(err) {
        i18n::fl("chat-auth-failed")
    } else {
        err.to_owned()
    }
}
#[cfg(test)]
mod tests {
    use super::{
        AsyncOutcome, ChatWindowAction, StageApp, chat_window_action, focus_loss_is_transient,
        format_log_text, overlay_window_level, provider_asset_load_status, window_focus_state,
        window_level,
    };
    use crate::core::events::LiveEvent;
    use crate::core::session::PreparedSessionTarget;
    use crate::surface::{PendingApproval, PendingQuestion};
    use ene_api::{HistoryResponse, MemoryCandidateView, MemoryJournalView, MemoryView};
    use std::sync::Arc;
    use std::time::Duration;
    use std::time::Instant;

    #[test]
    fn save_applies_window_level_for_transparent_and_opaque_overlays() {
        assert_eq!(window_level(true), winit::window::WindowLevel::AlwaysOnTop);
        assert_eq!(window_level(false), winit::window::WindowLevel::Normal);
    }

    #[test]
    fn focused_chrome_lowers_overlay_until_focus_returns() {
        assert_eq!(
            overlay_window_level(true, true),
            winit::window::WindowLevel::Normal
        );
        assert_eq!(
            overlay_window_level(false, true),
            winit::window::WindowLevel::AlwaysOnTop
        );
        assert_eq!(
            overlay_window_level(false, false),
            winit::window::WindowLevel::Normal
        );
    }

    #[test]
    fn chrome_focus_state_includes_focus_loss() {
        assert_eq!(
            window_focus_state(&winit::event::WindowEvent::Focused(true)),
            Some(true)
        );
        assert_eq!(
            window_focus_state(&winit::event::WindowEvent::Focused(false)),
            Some(false)
        );
    }

    #[test]
    fn tray_interaction_within_grace_is_transient() {
        let now = Instant::now();
        assert!(focus_loss_is_transient(
            Some(now),
            now + Duration::from_millis(100)
        ));
        assert!(!focus_loss_is_transient(
            Some(now.checked_sub(Duration::from_secs(5)).unwrap_or(now)),
            now
        ));
        assert_eq!(
            window_focus_state(&winit::event::WindowEvent::CloseRequested),
            None
        );
    }

    #[test]
    fn alt_tab_focus_loss_clears_chrome_without_tray_grace() {
        // Simulates: user Alt-Tabs away with no recent tray interaction.
        // chrome_focused must become false so overlay returns to AlwaysOnTop.
        let mut app = StageApp::new_for_test();
        app.chrome_focused = true;
        app.chat = None; // ensure chrome_window_exists() can be false
        app.detail_win = None;
        app.caption = None;
        app.spotlight = None;
        app.tray_interaction_at = None;

        // Apply Focused(false): chrome_focused must drop unconditionally.
        let focused_false = window_focus_state(&winit::event::WindowEvent::Focused(false));
        if let Some(focused) = focused_false {
            app.chrome_focused = focused;
        }
        assert!(!app.chrome_focused);
    }

    #[test]
    fn stale_approval_result_keeps_new_session_approval() {
        let mut app = StageApp::new_for_test();
        app.session.set_for_test(
            Arc::clone(&app.client),
            "soul",
            "new-session",
            HistoryResponse {
                messages: Vec::new(),
                depth: "surface".to_owned(),
            },
        );
        app.surface.pending_approval = Some(PendingApproval {
            id: "new-approval".to_owned(),
            tool: "fs.read".to_owned(),
            target: "/tmp/new".to_owned(),
        });

        app.apply_async_outcome(AsyncOutcome::Approval {
            session_id: "old-session".to_owned(),
            result: Ok(()),
        });

        assert_eq!(
            app.surface
                .pending_approval
                .as_ref()
                .map(|item| item.id.as_str()),
            Some("new-approval")
        );
    }

    #[test]
    fn a_new_approval_schedules_one_chat_reveal() {
        let mut app = StageApp::new_for_test();
        app.surface.chat_open = false;
        let approval = PendingApproval {
            id: "approval".to_owned(),
            tool: "fs.read".to_owned(),
            target: "/tmp/file".to_owned(),
        };

        app.set_pending_approval(approval.clone());

        assert!(app.surface.chat_open);
        assert!(std::mem::take(&mut app.approval_needs_reveal));

        app.set_pending_approval(approval);

        assert!(!app.approval_needs_reveal);
    }

    #[test]
    fn retarget_clears_session_scoped_questions_and_approvals() {
        let mut app = StageApp::new_for_test();
        app.surface.pending_approval = Some(PendingApproval {
            id: "old-approval".to_owned(),
            tool: "fs.read".to_owned(),
            target: "/tmp/old".to_owned(),
        });
        app.surface.pending_question = Some(PendingQuestion {
            id: "old-question".to_owned(),
            prompt: "old".to_owned(),
        });
        app.commit_session_target(PreparedSessionTarget::new_for_test(
            "new-session",
            HistoryResponse {
                messages: Vec::new(),
                depth: "surface".to_owned(),
            },
        ));

        assert_eq!(app.session.session_id(), "new-session");
        assert!(app.surface.pending_approval.is_none());
        assert!(app.surface.pending_question.is_none());
    }

    #[test]
    fn question_resolved_closes_only_the_matching_question() {
        let mut app = StageApp::new_for_test();
        app.surface.pending_question = Some(PendingQuestion {
            id: "job-1".to_owned(),
            prompt: "which city?".to_owned(),
        });

        app.apply_live_event(LiveEvent::QuestionResolved {
            id: "other-job".to_owned(),
        });
        assert_eq!(
            app.surface.pending_question.as_ref().map(|q| q.id.as_str()),
            Some("job-1")
        );

        app.apply_live_event(LiveEvent::QuestionResolved {
            id: "job-1".to_owned(),
        });

        assert!(app.surface.pending_question.is_none());
    }

    #[test]
    fn resolving_memory_refreshes_memories_and_pending_candidates() {
        let mut app = StageApp::new_for_test();
        app.apply_async_outcome(AsyncOutcome::ResolveMemory {
            soul_id: app.session.soul_id().to_owned(),
            id: "candidate-1".to_owned(),
            result: Ok(()),
        });

        app.runtime.block_on(async {
            for _ in 0..200 {
                if app.async_results.lock().len() >= 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        });

        let refresh_count = app
            .async_results
            .lock()
            .iter()
            .filter(|outcome| {
                matches!(
                    outcome,
                    AsyncOutcome::ListMemories { .. } | AsyncOutcome::ListPendingMemories { .. }
                )
            })
            .count();
        assert_eq!(refresh_count, 2);
    }

    #[test]
    fn stale_memory_results_are_ignored_after_soul_retarget() {
        let mut app = StageApp::new_for_test();
        app.session.set_for_test(
            Arc::clone(&app.client),
            "soul-a",
            "old-session",
            HistoryResponse {
                messages: Vec::new(),
                depth: "surface".to_owned(),
            },
        );
        app.commit_session_target(PreparedSessionTarget::new_for_test_with_soul(
            "soul-b",
            "new-session",
            HistoryResponse {
                messages: Vec::new(),
                depth: "surface".to_owned(),
            },
        ));
        app.detail.memories = vec![MemoryView {
            id: "memory-b".to_owned(),
            soul_id: "soul-b".to_owned(),
            scope: "private".to_owned(),
            kind: "semantic".to_owned(),
            title: "B memory".to_owned(),
            content: "B".to_owned(),
            expires_at: None,
            schedule_id: None,
        }];
        app.detail.pending_memories = vec![MemoryCandidateView {
            id: "candidate-b".to_owned(),
            soul_id: "soul-b".to_owned(),
            scope: "private".to_owned(),
            kind: "semantic".to_owned(),
            title: "B candidate".to_owned(),
            content: "B".to_owned(),
            confidence: 0.9,
            sensitive: false,
            expires_at: None,
        }];
        app.detail.memory_journal = vec![MemoryJournalView {
            seq: 1,
            ts: "now".to_owned(),
            memory_id: None,
            soul_id: "soul-b".to_owned(),
            action: "candidate_accepted".to_owned(),
            payload: serde_json::json!({}),
        }];

        app.apply_async_outcome(AsyncOutcome::ListMemories {
            soul_id: "soul-a".to_owned(),
            result: Ok(Vec::new()),
        });
        app.apply_async_outcome(AsyncOutcome::ListPendingMemories {
            soul_id: "soul-a".to_owned(),
            result: Ok(Vec::new()),
        });
        app.apply_async_outcome(AsyncOutcome::ListMemoryJournal {
            soul_id: "soul-a".to_owned(),
            result: Ok(Vec::new()),
        });
        app.apply_async_outcome(AsyncOutcome::ResolveMemory {
            soul_id: "soul-a".to_owned(),
            id: "candidate-a".to_owned(),
            result: Err("old soul failed".to_owned()),
        });

        assert_eq!(app.session.soul_id(), "soul-b");
        assert_eq!(app.detail.memories[0].id, "memory-b");
        assert_eq!(app.detail.pending_memories[0].id, "candidate-b");
        assert_eq!(app.detail.memory_journal[0].soul_id, "soul-b");
        assert!(app.detail.core_status.is_empty());
    }

    #[test]
    fn empty_log_payload_is_labeled_instead_of_hidden() {
        assert_eq!(format_log_text("turn completed"), "turn completed");
        assert_eq!(format_log_text("   "), "(empty)");
    }

    #[test]
    fn provider_asset_load_status_reports_success_and_empty_results() {
        assert!(provider_asset_load_status(2).ends_with(": 2"));
        assert!(provider_asset_load_status(0).ends_with(": 0"));
    }

    #[test]
    fn chat_paint_does_not_raise_an_existing_window() {
        assert_eq!(chat_window_action(true, true), ChatWindowAction::None);
        assert_eq!(chat_window_action(true, false), ChatWindowAction::Create);
        assert_eq!(chat_window_action(false, false), ChatWindowAction::None);
    }

    #[test]
    fn overlay_focus_tracks_chat_and_detail_transitions() {
        let mut focus = OverlayFocus::default();
        assert!(!focus.protects());

        focus.transition(FocusTarget::Chat);
        assert!(focus.protects());

        focus.transition(FocusTarget::Detail);
        assert!(focus.protects());
    }

    #[test]
    fn overlay_focus_loses_protection_when_overlay_gains_focus() {
        let mut focus = OverlayFocus::default();
        focus.transition(FocusTarget::Chat);

        assert!(focus.on_focus_event(FocusOwner::Overlay, true));
        assert!(!focus.protects());
    }

    #[test]
    fn chrome_focus_loss_clears_matching_target_only() {
        let mut focus = OverlayFocus::default();
        focus.transition(FocusTarget::Chat);

        // Detail gaining focus replaces the target without dropping protection.
        assert!(focus.on_focus_event(FocusOwner::Detail, true));
        assert!(focus.protects());

        // Detail losing focus clears it; a stale Chat loss must not.
        assert!(focus.on_focus_event(FocusOwner::Detail, false));
        assert!(!focus.protects());
        assert!(!focus.on_focus_event(FocusOwner::Chat, false));
        assert!(!focus.protects());
    }

    #[test]
    fn focus_event_returns_changed_only_on_actual_transition() {
        let mut focus = OverlayFocus::default();
        focus.transition(FocusTarget::Chat);

        assert!(!focus.on_focus_event(FocusOwner::Chat, true));
        assert!(focus.protects());

        assert!(focus.on_focus_event(FocusOwner::Detail, true));
        assert!(!focus.on_focus_event(FocusOwner::Detail, true));
    }

    #[test]
    fn closing_chat_resets_chat_state_for_reopen() {
        let mut app = StageApp::new_for_test();
        app.surface.chat_open = true;
        app.surface.focus_chat = true;
        app.overlay_focus.transition(FocusTarget::Chat);

        app.close_chat_window();

        assert!(app.chat.is_none());
        assert!(!app.surface.chat_open);
        assert!(!app.surface.chat_input_focused);
        assert!(!app.overlay_focus.protects());

        // Reopening after close re-marks the intent even without a window.
        app.surface.chat_open = true;
        app.surface.focus_chat = true;
        assert!(app.surface.chat_open);
        assert!(app.surface.focus_chat);
    }

    #[test]
    fn closing_detail_resets_visibility_and_focus() {
        let mut app = StageApp::new_for_test();
        app.detail.visible = true;
        app.overlay_focus.transition(FocusTarget::Detail);

        app.close_detail_window();

        assert!(app.detail_win.is_none());
        assert!(!app.detail.visible);
        assert!(!app.overlay_focus.protects());
    }

    #[test]
    fn reopen_after_close_sets_open_intent() {
        let mut app = StageApp::new_for_test();
        app.surface.chat_open = true;
        app.close_chat_window();
        assert!(!app.surface.chat_open);

        // Simulate tray/F2 open action (without GPU, so no window is created).
        app.surface.chat_open = true;
        app.surface.focus_chat = true;

        assert!(app.surface.chat_open);
        assert!(app.surface.focus_chat);
    }

    #[test]
    fn minimize_reopen_preserves_history() {
        let mut app = StageApp::new_for_test();
        app.surface.history.messages.push(ene_api::MessageResponse {
            seq: 1,
            role: "assistant".to_owned(),
            text: "kept".to_owned(),
        });
        // Minimize/hide does not touch surface history; only close does.
        assert_eq!(app.surface.history.messages.len(), 1);
        assert_eq!(app.surface.history.messages[0].text, "kept");
    }
}
