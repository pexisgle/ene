//! eframe application shell for the stage product client.

use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::Receiver;
use eframe::egui::{self, ViewportClass, ViewportId};
use ene_api::{ApiClient, HistoryResponse, SendMessageResponse};
use ene_config::EneConfigError;
use ene_vrm::viseme::VisemeAnalyzer;
use parking_lot::Mutex;
use thiserror::Error;
use tokio::runtime::{Handle, Runtime};

use crate::audio::AudioHub;
use crate::avatar::{look_at, AvatarError, VrmPane};
use crate::core::events::{spawn_event_listeners, LiveEvent};
use crate::core::session::StageSession;
use crate::core::spawn::{attach_or_spawn_core, StageCore, StageSpawnError};
use crate::detail::{self, DetailUiState};
use crate::i18n;
use crate::settings::{load_desktop_settings, save_desktop_settings, DesktopSettings};
use crate::shell::hotkeys::HotkeyError;
use crate::shell::tray::TrayError;
use crate::shell::{show_notification, HotkeyManager, ShellAction, ShellError, TrayAction, TrayManager};
use crate::surface::{self, SurfaceAction, SurfaceUiState};

const DETAIL_VIEWPORT: &str = "ene-stage-detail";

#[derive(Debug, Error)]
pub enum AppError {
    #[error("spawn: {0}")]
    Spawn(#[from] StageSpawnError),
    #[error("runtime: {0}")]
    Runtime(String),
    #[error("eframe: {0}")]
    Eframe(String),
    #[error("shell: {0}")]
    Shell(#[from] ShellError),
    #[error("config: {0}")]
    Config(#[from] EneConfigError),
    #[error("avatar: {0}")]
    Avatar(#[from] AvatarError),
}

pub fn run() -> Result<(), AppError> {
    let settings = load_desktop_settings();
    let runtime = Runtime::new().map_err(|err| AppError::Runtime(err.to_string()))?;
    let rt_handle = runtime.handle().clone();

    let (client, core, session, events) = runtime.block_on(async {
        let (client, core) = attach_or_spawn_core(&settings).await?;
        let client = Arc::new(client);
        let session = StageSession::bootstrap(Arc::clone(&client)).await?;
        let events = spawn_event_listeners(&client, session.session_id());
        Ok::<_, StageSpawnError>((client, core, session, events))
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
        Err(HotkeyError::Manager(err)) => {
            tracing::warn!(error = %err, "global hotkeys unavailable");
            None
        }
        Err(err) => return Err(ShellError::Hotkeys(err).into()),
    };

    let local_settings = settings.clone();
    apply_theme_from_settings(&local_settings.theme);

    let app = StageApp {
        settings,
        local_settings,
        core,
        session,
        client,
        runtime,
        rt_handle,
        events,
        audio: AudioHub::new(),
        vrm: None,
        vrm_path: None,
        look_at_state: look_at::LookAtState::default(),
        viseme: VisemeAnalyzer::new(AudioHub::new().sample_rate()),
        tray,
        hotkeys,
        surface: SurfaceUiState::default(),
        detail: DetailUiState::default(),
        async_results: Arc::new(Mutex::new(Vec::new())),
        mic_active: false,
        notify_claimed: false,
        vrm_load_requested: true,
    };

    let title = i18n::fl("app-title");
    let transparent = app.settings.transparent_overlay;
    let always_on_top = app.settings.always_on_top;

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(title)
        .with_transparent(transparent)
        .with_decorations(!transparent)
        .with_inner_size([480.0, 720.0]);
    if always_on_top {
        viewport = viewport.with_always_on_top();
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "ene-stage",
        native_options,
        Box::new(move |cc| Ok(Box::new(app.with_creation_context(cc)))),
    )
    .map_err(|err| AppError::Eframe(err.to_string()))?;

    Ok(())
}

struct StageApp {
    settings: DesktopSettings,
    local_settings: DesktopSettings,
    #[expect(dead_code, reason = "StageCore kills spawned ene-core on drop when lifetime is app")]
    core: StageCore,
    session: StageSession,
    client: Arc<ApiClient>,
    #[expect(dead_code, reason = "Tokio runtime must outlive the UI loop")]
    runtime: Runtime,
    rt_handle: Handle,
    events: Receiver<LiveEvent>,
    audio: AudioHub,
    vrm: Option<VrmPane>,
    vrm_path: Option<PathBuf>,
    look_at_state: look_at::LookAtState,
    viseme: VisemeAnalyzer,
    tray: Option<TrayManager>,
    hotkeys: Option<HotkeyManager>,
    surface: SurfaceUiState,
    detail: DetailUiState,
    async_results: Arc<Mutex<Vec<AsyncOutcome>>>,
    mic_active: bool,
    notify_claimed: bool,
    vrm_load_requested: bool,
}

impl StageApp {
    fn with_creation_context(self, cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx
            .set_visuals(theme_visuals(&self.local_settings.theme));
        self
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

    fn drain_async_results(&mut self) {
        let outcomes = {
            let mut guard = self.async_results.lock();
            std::mem::take(&mut *guard)
        };
        for outcome in outcomes {
            self.apply_async_outcome(outcome);
        }
    }

    fn apply_async_outcome(&mut self, outcome: AsyncOutcome) {
        match outcome {
            AsyncOutcome::SendMessage(result) => {
                if let Err(err) = result {
                    self.surface.status = err;
                } else {
                    self.surface.chat_draft.clear();
                    self.surface.streaming_text.clear();
                    self.request_history_refresh();
                }
            }
            AsyncOutcome::BargeIn(result) | AsyncOutcome::CancelTurn(result) => {
                if let Err(err) = result {
                    self.surface.status = err;
                }
            }
            AsyncOutcome::Approval(result) => {
                if result.is_ok() {
                    self.surface.pending_approval = None;
                } else if let Err(err) = result {
                    self.surface.status = err;
                }
            }
            AsyncOutcome::Listen(result) => {
                if let Err(err) = result {
                    tracing::debug!(error = %err, "listen failed");
                }
            }
            AsyncOutcome::RefreshHistory(result) => match result {
                Ok(history) => {
                    self.session.replace_history(history.clone());
                    self.surface.history = history;
                    self.surface.streaming_text.clear();
                }
                Err(err) => self.surface.status = err,
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
                    self.detail.core_status = i18n::fl("settings-loaded");
                }
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::ApplyCoreSettings(result) => {
                self.detail.core_status = match result {
                    Ok(()) => i18n::fl("settings-applied"),
                    Err(err) => err,
                };
            }
            AsyncOutcome::ListMemories(result) => match result {
                Ok(items) => self.detail.memories = items,
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::DeleteMemory { id, result } => {
                if result.is_ok() {
                    tracing::debug!(memory_id = %id, "memory deleted");
                    self.request_memories();
                } else if let Err(err) = result {
                    self.detail.core_status = err;
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
                    self.vrm = None;
                    self.vrm_load_requested = true;
                }
                Err(err) => self.detail.core_status = err,
            }
            AsyncOutcome::ImportCharacter(result) => match result {
                Ok(character) => {
                    self.detail.core_status = format!("{}: {}", i18n::fl("character-imported"), character.id);
                    self.request_characters();
                }
                Err(err) => self.detail.core_status = err,
            }
            AsyncOutcome::ListCharacters(result) => match result {
                Ok(items) => self.detail.characters = items,
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::ListJobs(result) => match result {
                Ok((jobs, schedules)) => {
                    self.detail.jobs = jobs;
                    self.detail.schedules = schedules;
                }
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::CancelJob { id, result } => {
                if result.is_ok() {
                    tracing::debug!(job_id = %id, "job cancelled");
                    self.request_jobs();
                } else if let Err(err) = result {
                    self.detail.core_status = err;
                }
            }
            AsyncOutcome::ToggleSchedule { id, enabled, result } => {
                if result.is_ok() {
                    tracing::debug!(schedule_id = %id, enabled, "schedule toggled");
                    self.request_jobs();
                } else if let Err(err) = result {
                    self.detail.core_status = err;
                }
            }
            AsyncOutcome::ListPlugins(result) => match result {
                Ok(items) => self.detail.plugins = items,
                Err(err) => self.detail.core_status = err,
            }
            AsyncOutcome::RestartPlugin { id, result } => {
                if result.is_ok() {
                    tracing::debug!(plugin_id = %id, "plugin restarted");
                    self.request_plugins();
                } else if let Err(err) = result {
                    self.detail.core_status = err;
                }
            }
            AsyncOutcome::ListProviderAssets(result) => match result {
                Ok(items) => self.detail.provider_assets = items,
                Err(err) => self.detail.core_status = err,
            }
            AsyncOutcome::InstallProviderAsset { asset_id, result } => match result {
                Ok(job_id) => {
                    self.detail
                        .provider_install_jobs
                        .insert(asset_id, job_id);
                    self.detail.core_status = i18n::fl("plugins-asset-install-started");
                }
                Err(err) => self.detail.core_status = err,
            }
            AsyncOutcome::ProviderAssetInstallStatus { asset_id, result } => match result {
                Ok(status) => {
                    if status.phase == Some(ene_api::ProviderAssetInstallPhase::Done) {
                        self.detail.provider_install_jobs.remove(&asset_id);
                        self.request_provider_assets();
                    } else if status.phase == Some(ene_api::ProviderAssetInstallPhase::Failed) {
                        self.detail.provider_install_jobs.remove(&asset_id);
                        self.detail.core_status = status
                            .error
                            .unwrap_or_else(|| i18n::fl("plugins-asset-install-failed"));
                    }
                }
                Err(err) => self.detail.core_status = err,
            }
            AsyncOutcome::SetActiveProviderAsset { asset_id, result } => {
                if result.is_ok() {
                    tracing::debug!(asset_id = %asset_id, "provider asset activated");
                    self.request_provider_assets();
                } else if let Err(err) = result {
                    self.detail.core_status = err;
                }
            }
            AsyncOutcome::LoadMcp(result) => match result {
                Ok(json) => self.detail.mcp_json = json,
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::SaveMcp(result) => {
                self.detail.core_status = match result {
                    Ok(()) => i18n::fl("mcp-saved"),
                    Err(err) => err,
                };
            }
            AsyncOutcome::MicClaim(result) => match result {
                Ok(active) => self.mic_active = active,
                Err(err) => self.surface.status = err,
            }
            AsyncOutcome::VrmPath(path) => {
                self.vrm_path = Some(path);
                self.vrm_load_requested = true;
            }
        }
    }

    fn request_memories(&self) {
        let soul_id = self.session.soul_id().to_owned();
        let client = Arc::clone(&self.client);
        self.spawn(async move {
            let result = client
                .list_memories(&soul_id, None)
                .await
                .map(|page| page.items)
                .map_err(|err| err.to_string());
            AsyncOutcome::ListMemories(result)
        });
    }

    fn request_characters(&self) {
        let client = Arc::clone(&self.client);
        self.spawn(async move {
            let result = client
                .list_characters()
                .await
                .map(|page| page.items)
                .map_err(|err| err.to_string());
            AsyncOutcome::ListCharacters(result)
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

    fn request_plugins(&self) {
        let client = Arc::clone(&self.client);
        self.spawn(async move {
            let result = client
                .list_plugins()
                .await
                .map(|page| page.items)
                .map_err(|err| err.to_string());
            AsyncOutcome::ListPlugins(result)
        });
    }

    fn request_provider_assets(&self) {
        let plugin = self.detail.provider_assets_plugin.clone();
        if plugin.is_empty() {
            return;
        }
        let client = Arc::clone(&self.client);
        self.spawn(async move {
            let result = client
                .list_provider_assets(&ene_api::ListProviderAssetsRequest { plugin })
                .await
                .map(|response| response.assets)
                .map_err(|err| err.to_string());
            AsyncOutcome::ListProviderAssets(result)
        });
    }

    fn request_history_refresh(&self) {
        let session = self.session.clone_handle();
        self.spawn(async move {
            let result = session
                .refresh_history()
                .await
                .map_err(|err| err.to_string());
            AsyncOutcome::RefreshHistory(result)
        });
    }

    fn toggle_mic(&mut self) {
        let session = self.session.clone_handle();
        let enable = !self.mic_active;
        self.spawn(async move {
            let result = if enable {
                session.claim_mic().await.map(|_| true).map_err(|e| e.to_string())
            } else {
                session.release_mic().await.map(|_| false).map_err(|e| e.to_string())
            };
            AsyncOutcome::MicClaim(result)
        });
    }

    fn send_chat(&mut self) {
        let text = self.surface.chat_draft.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let session = self.session.clone_handle();
        self.spawn(async move {
            let result = session
                .send_prompt(&text)
                .await
                .map(|_| ())
                .map_err(|err| err.to_string());
            AsyncOutcome::SendMessage(result)
        });
    }

    fn barge_in(&mut self) {
        let session = self.session.clone_handle();
        self.spawn(async move {
            let result = session.barge_in().await.map(|_| ()).map_err(|e| e.to_string());
            AsyncOutcome::BargeIn(result)
        });
    }

    fn cancel_turn(&mut self) {
        let session = self.session.clone_handle();
        self.spawn(async move {
            let result = session
                .cancel_turn()
                .await
                .map(|_| ())
                .map_err(|e| e.to_string());
            AsyncOutcome::CancelTurn(result)
        });
    }

    fn respond_approval(&mut self, id: &str, decision: &str) {
        let session = self.session.clone_handle();
        let id = id.to_owned();
        let decision = decision.to_owned();
        self.spawn(async move {
            let result = session
                .respond_approval(&id, &decision)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string());
            AsyncOutcome::Approval(result)
        });
    }

    fn save_local_settings(&mut self) {
        let settings = self.local_settings.clone();
        self.settings = settings.clone();
        apply_theme_from_settings(&settings.theme);
        self.spawn(async move {
            let result = save_desktop_settings(&settings).map_err(|err| err.to_string());
            AsyncOutcome::SaveLocalSettings(result)
        });
    }

    fn persist_character_position(&mut self) {
        self.local_settings.character_x = self.surface.character_pos[0];
        self.local_settings.character_y = self.surface.character_pos[1];
        let settings = self.local_settings.clone();
        self.settings = settings.clone();
        self.spawn(async move {
            let result = save_desktop_settings(&settings).map_err(|err| err.to_string());
            AsyncOutcome::SaveLocalSettings(result)
        });
    }

    fn poll_shell(&mut self, ctx: &egui::Context) {
        let tray_actions: Vec<TrayAction> = self
            .tray
            .as_ref()
            .map(|tray| {
                let mut actions = Vec::new();
                while let Some(action) = tray.try_recv() {
                    actions.push(action);
                }
                actions
            })
            .unwrap_or_default();
        for action in tray_actions {
            match action {
                TrayAction::OpenDetail => self.detail.visible = true,
                TrayAction::OpenChatFocus => self.surface.focus_chat = true,
                TrayAction::ToggleMic => self.toggle_mic(),
                TrayAction::Quit => self.surface.quit = true,
            }
        }
        if let Some(hotkeys) = self.hotkeys.as_ref()
            && let Some(action) = hotkeys.poll()
        {
            match action {
                ShellAction::OpenSpotlight => {
                    if self.settings.spotlight_enabled {
                        self.surface.spotlight_open = true;
                    }
                }
                ShellAction::OpenDetail => self.detail.visible = true,
                ShellAction::FocusChat => self.surface.focus_chat = true,
                ShellAction::ToggleMic => self.toggle_mic(),
                ShellAction::Quit => self.surface.quit = true,
            }
        }
        if self.surface.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn process_surface_actions(&mut self) {
        let actions = std::mem::take(&mut self.surface.pending_actions);
        for action in actions {
            match action {
                SurfaceAction::SendChat => self.send_chat(),
                SurfaceAction::BargeIn => self.barge_in(),
                SurfaceAction::CancelTurn => self.cancel_turn(),
                SurfaceAction::ToggleMic => self.toggle_mic(),
                SurfaceAction::Approval { decision } => {
                    if let Some(pending) = self.surface.pending_approval.clone() {
                        self.respond_approval(&pending.id, &decision);
                    }
                }
                SurfaceAction::OpenDetail(tab) => {
                    self.detail.visible = true;
                    self.detail.tab = tab;
                }
                SurfaceAction::Quit => self.surface.quit = true,
                SurfaceAction::PersistCharacterPos => self.persist_character_position(),
            }
        }
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                LiveEvent::TextDelta { text, .. } => {
                    self.surface.streaming_text.push_str(&text);
                    if self.settings.caption_enabled {
                        self.surface.caption = self.surface.streaming_text.clone();
                    }
                }
                LiveEvent::ThinkingDelta { text } | LiveEvent::InnerMessage { text } => {
                    self.detail.push_log(detail::LogKind::Thinking, text);
                }
                LiveEvent::ToolCall { summary } => {
                    self.detail.push_log(detail::LogKind::Tool, summary);
                }
                LiveEvent::SessionEvent { kind, text } => {
                    self.detail
                        .push_log(detail::LogKind::Session, format!("{kind}: {text}"));
                    if kind == "turn/end" {
                        self.request_history_refresh();
                        self.surface.streaming_text.clear();
                        if self.settings.caption_enabled {
                            self.surface.caption.clear();
                        }
                    }
                }
                LiveEvent::ApprovalAsked { id, tool, target } => {
                    self.surface.pending_approval = Some(surface::PendingApproval { id, tool, target });
                }
                LiveEvent::ApprovalResolved { .. } => {
                    self.surface.pending_approval = None;
                }
                LiveEvent::NotifyHint { title, body } => {
                    if let Err(err) = show_notification(&title, &body, "ene-stage") {
                        tracing::debug!(error = %err, "notification failed");
                    }
                }
                LiveEvent::BodyCommand { value } => {
                    if let Some(vrm) = self.vrm.as_mut() {
                        vrm.avatar_mut().apply_body_event(&value);
                    }
                }
                LiveEvent::AudioChunk { pcm, sample_rate } => {
                    if let Err(err) = self.audio.play_pcm(&pcm, sample_rate) {
                        tracing::debug!(error = %err, "audio playback failed");
                    }
                }
                LiveEvent::AffectState { mood_label, .. } => {
                    self.surface.status = mood_label;
                }
                LiveEvent::JobReport { text } => {
                    self.detail.push_log(detail::LogKind::Job, text);
                }
                LiveEvent::Disconnected => {
                    self.surface.status = i18n::fl("status-disconnected");
                }
            }
            ctx.request_repaint();
        }
    }

    fn poll_audio(&mut self) {
        if self.audio.mic_barge_in() {
            self.barge_in();
        }
        if !self.mic_active {
            return;
        }
        let sample_rate = self.audio.sample_rate();
        for chunk in self.audio.poll_mic_chunks() {
            let session = self.session.clone_handle();
            self.spawn(async move {
                let result = session
                    .listen_pcm(chunk, sample_rate)
                    .await
                    .map(|_| ())
                    .map_err(|err| err.to_string());
                AsyncOutcome::Listen(result)
            });
        }
    }

    fn ensure_notify(&mut self) {
        if self.notify_claimed {
            return;
        }
        let session = self.session.clone_handle();
        self.notify_claimed = true;
        self.spawn(async move {
            if let Err(err) = session.claim_notify().await {
                tracing::debug!(error = %err, "notify claim failed");
            }
            AsyncOutcome::MicClaim(Ok(false))
        });
    }

    fn ensure_vrm(&mut self) {
        if self.vrm.is_some() || !self.vrm_load_requested {
            return;
        }
        self.vrm_load_requested = false;
        let client = Arc::clone(&self.client);
        let soul_id = self.session.soul_id().to_owned();
        let rt = self.rt_handle.clone();
        let results = Arc::clone(&self.async_results);
        rt.spawn(async move {
            let path = resolve_vrm_path(&client, &soul_id).await;
            results.lock().push(AsyncOutcome::VrmPath(path));
        });
    }

    fn try_load_vrm(
        &mut self,
        ctx: &egui::Context,
        render_state: &eframe::egui_wgpu::RenderState,
        path: &std::path::Path,
    ) {
        let width = 512;
        let height = 768;
        match VrmPane::load(
            path,
            &render_state.device,
            &render_state.queue,
            render_state.target_format,
            width,
            height,
        ) {
            Ok(pane) => {
                self.vrm = Some(pane);
                tracing::info!(path = %path.display(), "loaded VRM avatar");
            }
            Err(err) => {
                tracing::warn!(error = %err, path = %path.display(), "VRM load failed");
            }
        }
        ctx.request_repaint();
    }

    fn tick_avatar(
        &mut self,
        ctx: &egui::Context,
        render_state: &eframe::egui_wgpu::RenderState,
        dt: f32,
    ) {
        let Some(vrm) = self.vrm.as_mut() else {
            return;
        };
        let weights = self.audio.analyze_visemes(&mut self.viseme);
        vrm.avatar_mut().apply_viseme(weights);

        let pointer = ctx.input(|i| i.pointer.interact_pos());
        let screen = ctx.content_rect();
        let viewport = (
            screen.width().max(1.0) as u32,
            screen.height().max(1.0) as u32,
        );
        if let Some(cursor) = pointer {
            let avatar = vrm.avatar_mut();
            let (eye, target) = {
                let cam = avatar.camera();
                (
                    glam::Vec3::from(cam.eye()),
                    glam::Vec3::from(cam.target()),
                )
            };
            let head = avatar.head_world();
            let up = glam::Vec3::from(ene_vrm::camera::DEFAULT_UP);
            let cursor_logical = glam::Vec2::new(cursor.x, cursor.y);
            let world = look_at::compute_world_target(
                cursor_logical,
                viewport,
                eye,
                target,
                up,
                head,
                self.local_settings.look_at_strength,
                &mut self.look_at_state,
                dt,
            );
            avatar.set_look_at_target(world);
        }

        if let Err(err) =
            vrm.tick_ui_frame(ctx, &render_state.device, &render_state.queue, dt)
        {
            tracing::debug!(error = %err, "avatar tick failed");
        }
    }

    fn show_detail_viewport(&mut self, ctx: &egui::Context) {
        if !self.detail.visible {
            return;
        }
        let mut open = self.detail.visible;
        let title = i18n::fl("detail-title");
        let visible = self.detail.visible;
        let mut detail = std::mem::take(&mut self.detail);
        detail.visible = visible;
        let mut local_settings = self.local_settings.clone();
        let client = Arc::clone(&self.client);
        let rt = self.rt_handle.clone();
        let results = Arc::clone(&self.async_results);
        let soul_id = self.session.soul_id().to_owned();
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of(DETAIL_VIEWPORT),
            egui::ViewportBuilder::default()
                .with_title(title)
                .with_inner_size([900.0, 640.0]),
            |ui, class| {
                if !matches!(class, ViewportClass::Immediate | ViewportClass::EmbeddedWindow) {
                    return;
                }
                egui::CentralPanel::default().show(ui, |ui| {
                    detail::show(
                        ui,
                        &mut detail,
                        &mut local_settings,
                        &soul_id,
                        &client,
                        &rt,
                        &results,
                    );
                });
                if ui.ctx().input(|i| i.viewport().close_requested()) {
                    open = false;
                }
            },
        );
        detail.visible = open;
        self.detail = detail;
        self.local_settings = local_settings;
        if self.detail.save_local_pending {
            self.detail.save_local_pending = false;
            self.save_local_settings();
        }
    }
}

impl eframe::App for StageApp {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        ctx.set_visuals(theme_visuals(&self.local_settings.theme));

        self.drain_async_results();
        self.drain_events(ctx);
        self.poll_shell(ctx);
        self.poll_audio();
        self.ensure_notify();
        self.process_surface_actions();

        if self.surface.history.messages.is_empty() {
            self.surface.history = self.session.history();
        }

        if let Some(render_state) = frame.wgpu_render_state() {
            self.ensure_vrm();
            if let Some(path) = self.vrm_path.take() {
                self.try_load_vrm(ctx, render_state, &path);
            }
            let dt = ctx.input(|i| i.stable_dt);
            self.tick_avatar(ctx, render_state, dt);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        surface::show(
            ui,
            &mut self.surface,
            &self.settings,
            self.vrm.as_ref(),
            self.mic_active,
        );
        self.process_surface_actions();
        self.show_detail_viewport(ui.ctx());
        let _ = frame;
    }
}

async fn resolve_vrm_path(client: &ApiClient, soul_id: &str) -> PathBuf {
    if let Ok(soul) = client.get_soul(soul_id).await
        && let Some(body_ref) = soul.body_ref.filter(|p| !p.is_empty())
    {
        let path = PathBuf::from(body_ref);
        if path.is_file() {
            return path;
        }
    }
    let dir = crate::platform::preferred_data_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        tracing::warn!(dir = %dir.display(), "failed to create avatar data dir");
    }
    let path = dir.join("default.vrm");
    if !path.is_file()
        && let Err(err) = crate::avatar::CompanionAvatar::write_default_minimal_vrm(&path)
    {
        tracing::warn!(error = %err, "failed to write default VRM");
    }
    path
}

fn apply_theme_from_settings(theme: &str) {
    // egui theme is applied each frame from local settings
    let _ = theme;
}

fn theme_visuals(theme: &str) -> egui::Visuals {
    match theme {
        "light" => egui::Visuals::light(),
        "dark" => egui::Visuals::dark(),
        _ => {
            if cfg!(target_os = "windows") {
                egui::Visuals::light()
            } else {
                egui::Visuals::dark()
            }
        }
    }
}

pub enum AsyncOutcome {
    SendMessage(Result<(), String>),
    BargeIn(Result<(), String>),
    CancelTurn(Result<(), String>),
    Approval(Result<(), String>),
    Listen(Result<(), String>),
    RefreshHistory(Result<HistoryResponse, String>),
    SaveLocalSettings(Result<(), String>),
    LoadCoreSettings(Result<String, String>),
    ApplyCoreSettings(Result<(), String>),
    ListMemories(Result<Vec<ene_api::MemoryView>, String>),
    DeleteMemory {
        id: String,
        result: Result<(), String>,
    },
    LoadSoul(Result<ene_api::SoulView, String>),
    PatchBody(Result<ene_api::SoulView, String>),
    ImportCharacter(Result<ene_api::CharacterView, String>),
    ListCharacters(Result<Vec<ene_api::CharacterView>, String>),
    ListJobs(Result<(Vec<ene_api::JobView>, Vec<ene_api::ScheduleView>), String>),
    CancelJob {
        id: String,
        result: Result<(), String>,
    },
    ToggleSchedule {
        id: String,
        enabled: bool,
        result: Result<(), String>,
    },
    ListPlugins(Result<Vec<ene_api::PluginView>, String>),
    RestartPlugin {
        id: String,
        result: Result<(), String>,
    },
    ListProviderAssets(Result<Vec<ene_api::ProviderAssetView>, String>),
    InstallProviderAsset {
        asset_id: String,
        result: Result<String, String>,
    },
    ProviderAssetInstallStatus {
        asset_id: String,
        result: Result<ene_api::ProviderAssetInstallStatusResponse, String>,
    },
    SetActiveProviderAsset {
        asset_id: String,
        result: Result<(), String>,
    },
    LoadMcp(Result<String, String>),
    SaveMcp(Result<(), String>),
    MicClaim(Result<bool, String>),
    VrmPath(PathBuf),
}

impl StageSession {
    fn clone_handle(&self) -> SessionHandle {
        SessionHandle {
            client: Arc::clone(self.client()),
            soul_id: self.soul_id().to_owned(),
            session_id: self.session_id().to_owned(),
            turn_id: Arc::new(Mutex::new(self.turn_id())),
            history: Arc::new(Mutex::new(self.history())),
        }
    }
}

#[derive(Clone)]
struct SessionHandle {
    client: Arc<ApiClient>,
    soul_id: String,
    session_id: String,
    turn_id: Arc<Mutex<Option<String>>>,
    history: Arc<Mutex<HistoryResponse>>,
}

impl SessionHandle {
    async fn refresh_history(&self) -> Result<HistoryResponse, ene_api::ApiError> {
        tracing::trace!(soul_id = %self.soul_id, "refresh surface history");
        let history = self.client.history(&self.session_id, "surface").await?;
        *self.history.lock() = history.clone();
        Ok(history)
    }

    async fn send_prompt(&self, text: &str) -> Result<SendMessageResponse, ene_api::ApiError> {
        let response = self
            .client
            .send_message(
                &self.session_id,
                &ene_api::MessageRequest {
                    text: text.to_owned(),
                    mode: ene_api::MessageMode::Prompt,
                    input_modality: None,
                },
                None,
            )
            .await?;
        if let Some(turn_id) = response.turn_id.clone() {
            *self.turn_id.lock() = Some(turn_id);
        }
        Ok(response)
    }

    async fn barge_in(&self) -> Result<serde_json::Value, ene_api::ApiError> {
        self.client.barge_in(&self.session_id).await
    }

    async fn cancel_turn(&self) -> Result<serde_json::Value, ene_api::ApiError> {
        let turn_id = self
            .turn_id
            .lock()
            .clone()
            .ok_or_else(|| ene_api::ApiError::Transport("no active turn".to_owned()))?;
        self.client.cancel_turn(&turn_id).await
    }

    async fn respond_approval(&self, id: &str, decision: &str) -> Result<serde_json::Value, ene_api::ApiError> {
        self.client.respond_approval(id, decision).await
    }

    async fn listen_pcm(
        &self,
        pcm: Vec<f32>,
        sample_rate: u32,
    ) -> Result<SendMessageResponse, ene_api::ApiError> {
        let response = self
            .client
            .listen(
                &self.session_id,
                &ene_api::ListenRequest { pcm, sample_rate },
            )
            .await?;
        if let Some(turn_id) = response.turn_id.clone() {
            *self.turn_id.lock() = Some(turn_id);
        }
        Ok(response)
    }

    async fn claim_mic(&self) -> Result<ene_api::ExclusiveSnapshot, ene_api::ApiError> {
        self.client
            .claim_resource(
                ene_api::ResourceKind::Mic,
                &ene_api::ClaimResourceRequest {
                    client_id: self.client.client_id().to_owned(),
                },
            )
            .await
    }

    async fn release_mic(&self) -> Result<ene_api::ExclusiveSnapshot, ene_api::ApiError> {
        self.client.release_resource(ene_api::ResourceKind::Mic).await
    }

    async fn claim_notify(&self) -> Result<ene_api::ExclusiveSnapshot, ene_api::ApiError> {
        self.client
            .claim_resource(
                ene_api::ResourceKind::Notify,
                &ene_api::ClaimResourceRequest {
                    client_id: self.client.client_id().to_owned(),
                },
            )
            .await
    }
}

