//! winit application handler for the product stage client.

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ene_api::{CreateJobRequest, MessageMode, SoulPatch};
use ene_vrm::viseme::VisemeAnalyzer;
use parking_lot::Mutex;
use thiserror::Error;
use tokio::runtime::{Handle, Runtime};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, OwnedDisplayHandle};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

use crate::audio::{AudioHub, ListenAction, MicListen, SendResult, run_listen_stream};
use crate::avatar::look_at;
use crate::chrome::{ChromeKind, ChromeWindow};
use crate::core::events::{EventFeeds, LiveEvent, spawn_event_feeds};
use crate::core::session::{StageSession, prepare_soul_target, send_direct_interaction};
use crate::core::spawn::{StageCore, StageSpawnError, attach_or_spawn_core};
use crate::detail::{self, DetailTab, DetailUiState, DisplayAction, LogKind, PendingJobRetry};
use crate::gpu::{GpuContext, GpuError};
use crate::i18n;
use crate::interaction::{EndResult, GestureTracker, PointerKind, ReactionKind};
use crate::interaction_controller::{
    CancelReason, InteractionSnapshot, StageInteractionController, UiInteractionRequest,
};
use crate::monitor::{self, MonitorInfo, OverlayMonitorMode};
use crate::overlay::{AvatarLoad, OverlayError, OverlayWindow};
use crate::platform::OverlayPlatform;
use crate::renderer::slint_gpu::{ChromeAction, OverlayUiAction};
use crate::scene::{
    AvatarPart, InteractionPrimitive, PxRect, SceneHit, StageAnchor, StageAnchorKind, StageScene,
    VisualPrimitive,
};
use crate::settings::{
    DesktopSettings, load_desktop_settings, normalize_displayed_souls, ordered_visible_souls,
    save_desktop_settings,
};
use crate::shell::tray::TrayError;
use crate::shell::{HotkeyManager, ShellCommand, ShellError, TrayManager, show_notification};
use crate::surface::chat::{
    chat_setup_cta_eligible, composer_send_allowed, normalize_transcript, transcript_label,
};
use crate::surface::{self, SpotlightAction, SurfaceAction, SurfaceUiState};
use crate::tasks::AsyncOutcome;
use slint::Global;

/// Bounded number of turn-end refresh retries while the projection catches up
/// with the completed turn.
const MAX_COMPLETION_REFRESHES: u32 = 3;

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
/// A chrome `Focused(false)` starts a short grace period during which the
/// overlay stays protected; another chrome `Focused(true)` cancels it, so a
/// normal chrome-to-chrome/OS focus handoff never exposes the overlay even
/// though the loss event always arrives before the next gain event.
#[derive(Debug, Default)]
struct OverlayFocus {
    target: Option<FocusTarget>,
    pending_loss_until: Option<Instant>,
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
                    self.mark_pending_loss()
                }
            }
            FocusOwner::Detail => {
                if focused {
                    self.set(FocusTarget::Detail)
                } else {
                    self.mark_pending_loss()
                }
            }
            FocusOwner::Caption => {
                if focused {
                    self.set(FocusTarget::Caption)
                } else {
                    self.mark_pending_loss()
                }
            }
            FocusOwner::Spotlight => {
                if focused {
                    self.set(FocusTarget::Spotlight)
                } else {
                    self.mark_pending_loss()
                }
            }
        }
    }

    fn set(&mut self, target: FocusTarget) -> bool {
        let changed = self.target != Some(target);
        self.target = Some(target);
        self.cancel_pending_loss();
        changed
    }

    fn mark_pending_loss(&mut self) -> bool {
        if self.target.is_none() {
            return false;
        }
        self.pending_loss_until = Some(Instant::now() + FOCUS_LOSS_GRACE);
        // Protection intentionally stays until the grace expires or another
        // chrome claim cancels it; no interaction sync must run yet.
        false
    }

    /// Drop protection once a pending focus loss outlives its grace period.
    /// Returns whether the state changed and a sync is required.
    fn expire_pending_loss(&mut self, now: Instant) -> bool {
        let Some(deadline) = self.pending_loss_until else {
            return false;
        };
        if now < deadline {
            return false;
        }
        self.pending_loss_until = None;
        self.clear()
    }

    fn clear(&mut self) -> bool {
        let had = self.target.is_some();
        self.pending_loss_until = None;
        self.target = None;
        had
    }

    fn clear_target(&mut self, target: FocusTarget) {
        if self.target == Some(target) {
            self.clear();
        } else {
            self.cancel_pending_loss();
        }
    }

    fn cancel_pending_loss(&mut self) {
        self.pending_loss_until = None;
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
    if settings.overlay_click_through
        && std::env::var("WAYLAND_DISPLAY").is_ok_and(|d| !d.is_empty())
    {
        // Wayland delivers no pointer events to a click-through surface, so
        // per-body dragging needs the preference turned off first.
        tracing::info!(
            "Wayland: drag bodies with System -> Overlay click-through off (Space shows the frame)"
        );
    }
    let runtime = Runtime::new().map_err(|err| AppError::Runtime(err.to_string()))?;
    let rt_handle = runtime.handle().clone();

    // Slint chrome shares the wgpu device; keep the Wayland display handle
    // alive for the lifetime of the event loop.
    let event_loop = EventLoop::new().map_err(|err| AppError::Window(err.to_string()))?;
    event_loop.set_control_flow(ControlFlow::WaitUntil(
        Instant::now() + Duration::from_millis(16),
    ));
    let _wayland_display: OwnedDisplayHandle = event_loop.owned_display_handle();

    let (client, core, session, feeds) = runtime.block_on(async {
        let (client, core) = attach_or_spawn_core(&settings).await?;
        let client = Arc::new(client);
        let session = StageSession::bootstrap(Arc::clone(&client)).await?;
        let feeds = spawn_event_feeds(&rt_handle, &client, session.session_id());
        Ok::<_, StageSpawnError>((client, core, session, feeds))
    })?;

    let tray = match TrayManager::new(&rt_handle) {
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
        temporarily_hidden_souls: HashSet::new(),
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
        interaction: StageInteractionController::default(),
        scene: StageScene::new(),
        overlay_platform: OverlayPlatform::default(),
        last_cursor: None,
        gesture: GestureTracker::default(),
        cursor_poll: None,
        last_global_cursor: None,
        monitors: Vec::new(),
        next_monitor_probe: Instant::now(),
        direct_reaction_agent_inflight: false,
        direct_reaction_retarget_inflight: false,
        last_tick: Instant::now(),
        last_approval_poll: Instant::now(),
        approval_poll_inflight: false,
        approval_needs_reveal: false,
        pending_completion_refreshes: 0,
        completion_reconcile_inflight: false,
        completion_terminal_seq: None,
        pending_optimistic_user_rows: Vec::new(),
        local_settings_save_generation: 0,
        last_overlay_slint: None,
        last_chat_sync: None,
        last_detail_sync: None,
        last_caption_sync: None,
        last_spotlight_sync: None,
    };
    app.surface.history = app.session.history();
    app.surface.greetings = app.session.greetings().to_vec();
    app.surface.chat_setup = app.detail.clone();
    detail::ensure_settings(
        &mut app.detail,
        &app.client,
        &app.rt_handle,
        &app.async_results,
    );
    // Load the active soul once at boot so the Home readiness cards and the
    // companion list reflect the live companion without first opening the
    // Companion tab (#1177). The Companion tab re-issues this idempotently.
    app.request_active_soul();
    app.claim_speaker_notify();

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
    temporarily_hidden_souls: HashSet<String>,
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
    interaction: StageInteractionController,
    scene: StageScene,
    overlay_platform: OverlayPlatform,
    last_cursor: Option<PhysicalPosition<f32>>,
    gesture: GestureTracker,
    cursor_poll: Option<crossbeam_channel::Receiver<crate::cursor_poll::GlobalCursor>>,
    last_global_cursor: Option<crate::cursor_poll::GlobalCursor>,
    monitors: Vec<MonitorInfo>,
    next_monitor_probe: Instant,
    direct_reaction_agent_inflight: bool,
    direct_reaction_retarget_inflight: bool,
    last_tick: Instant,
    last_approval_poll: Instant,
    approval_poll_inflight: bool,
    approval_needs_reveal: bool,
    pending_completion_refreshes: u32,
    completion_reconcile_inflight: bool,
    completion_terminal_seq: Option<u64>,
    pending_optimistic_user_rows: Vec<OptimisticUserRow>,
    local_settings_save_generation: u64,
    last_overlay_slint: Option<OverlaySlintKey>,
    last_chat_sync: Option<u64>,
    last_detail_sync: Option<u64>,
    last_caption_sync: Option<u64>,
    last_spotlight_sync: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
struct OverlaySlintKey {
    caption: String,
    speaker: String,
    notice: String,
    greetings: Vec<(String, String)>,
    head: Option<(i32, i32)>,
}

#[derive(Debug)]
struct OptimisticUserRow {
    seq: u64,
    text: String,
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
            temporarily_hidden_souls: HashSet::new(),
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
            interaction: StageInteractionController::default(),
            scene: StageScene::new(),
            overlay_platform: OverlayPlatform::default(),
            last_cursor: None,
            gesture: GestureTracker::default(),
            cursor_poll: None,
            last_global_cursor: None,
            monitors: Vec::new(),
            next_monitor_probe: Instant::now(),
            direct_reaction_agent_inflight: false,
            direct_reaction_retarget_inflight: false,
            last_tick: Instant::now(),
            last_approval_poll: Instant::now(),
            approval_poll_inflight: false,
            approval_needs_reveal: false,
            pending_completion_refreshes: 0,
            completion_reconcile_inflight: false,
            completion_terminal_seq: None,
            pending_optimistic_user_rows: Vec::new(),
            local_settings_save_generation: 0,
            last_overlay_slint: None,
            last_chat_sync: None,
            last_detail_sync: None,
            last_caption_sync: None,
            last_spotlight_sync: None,
        }
    }

    fn chrome_window_exists(&self) -> bool {
        self.chat.is_some()
            || self.detail_win.is_some()
            || self.caption.is_some()
            || self.spotlight.is_some()
    }

    fn chat_model_revision(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.surface.history.messages.len().hash(&mut hasher);
        if let Some(last) = self.surface.history.messages.last() {
            last.seq.hash(&mut hasher);
            last.role.hash(&mut hasher);
            last.text.hash(&mut hasher);
        }
        self.surface.streaming_text.hash(&mut hasher);
        self.surface.status.hash(&mut hasher);
        self.surface.turn_active.hash(&mut hasher);
        self.mic_active.hash(&mut hasher);
        self.surface
            .pending_approval
            .as_ref()
            .map(|pending| pending.id.as_str())
            .hash(&mut hasher);
        self.surface
            .pending_question
            .as_ref()
            .map(|question| question.prompt.as_str())
            .hash(&mut hasher);
        self.local_settings.theme.hash(&mut hasher);
        self.session.soul_id().hash(&mut hasher);
        hasher.finish()
    }

    fn detail_model_revision(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        std::mem::discriminant(&self.detail.tab).hash(&mut hasher);
        self.detail.search.hash(&mut hasher);
        self.detail.log.len().hash(&mut hasher);
        self.detail.core_status.hash(&mut hasher);
        self.detail.health.hash(&mut hasher);
        self.detail.chat_plugin.hash(&mut hasher);
        self.detail.chat_model.hash(&mut hasher);
        self.local_settings.theme.hash(&mut hasher);
        self.local_settings.displayed_soul_ids.hash(&mut hasher);
        hasher.finish()
    }

    fn caption_model_revision(&self, text: &str, font: i32, position: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        font.hash(&mut hasher);
        position.hash(&mut hasher);
        self.local_settings.theme.hash(&mut hasher);
        hasher.finish()
    }

    fn spotlight_model_revision(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.surface.spotlight_query.hash(&mut hasher);
        self.surface.spotlight_selected.hash(&mut hasher);
        self.local_settings.theme.hash(&mut hasher);
        hasher.finish()
    }

    /// Converts a screen-space pointer position to overlay-local physical
    /// coordinates using the overlay window's outer position.
    /// Returns `None` when the overlay window is gone.
    fn global_cursor_to_physical(
        &self,
        cursor: crate::cursor_poll::GlobalCursor,
    ) -> Option<PhysicalPosition<f32>> {
        let overlay = self.overlay.as_ref()?;
        let pos = overlay.window.outer_position().ok()?;
        Some(PhysicalPosition::new(cursor.x - f64::from(pos.x), cursor.y - f64::from(pos.y)).cast())
    }

    fn sync_overlay_interaction(&mut self) {
        // A tray menu steal is not a real focus switch; the overlay-focus
        // state machine already models that grace window via its target, so
        // protection derives solely from which chrome surface owns focus.
        let protect_chrome = self.overlay_focus.protects();
        let always_on_top = self.local_settings.always_on_top;
        let preferred_click_through = self.local_settings.overlay_click_through;
        let overlay_transparent = self.overlay.as_ref().is_some_and(|o| o.transparent);
        let window_visible = self
            .overlay
            .as_ref()
            .is_some_and(|o| o.window.is_visible() != Some(false));
        let has_avatar = self
            .overlay
            .as_ref()
            .is_some_and(OverlayWindow::has_avatars);

        if let Some(cursor) = self
            .cursor_poll
            .as_ref()
            .and_then(|rx| rx.try_iter().last())
        {
            self.last_global_cursor = Some(cursor);
        }
        if self.last_global_cursor.is_none()
            && let Some([x, y]) = crate::platform::global_cursor_position()
        {
            self.last_global_cursor = Some(crate::cursor_poll::GlobalCursor {
                x: f64::from(x),
                y: f64::from(y),
            });
        }

        // Passive windows receive no pointer events. Re-arm hover from a
        // global cursor poll (X11 thread or Windows GetCursorPos). This is
        // not an OS-region hover-rearm; Wayland uses input regions instead.
        if !self.interaction.cursor_hittest_enabled() {
            let polled = self
                .last_global_cursor
                .and_then(|cursor| self.global_cursor_to_physical(cursor));
            if let Some(physical) = polled {
                self.last_cursor = Some(physical);
                if let Some((eye, target, up, vw, vh)) = self.camera_basis() {
                    let candidates = self.overlay_hit_candidates();
                    let hit = crate::drag::hit_test(
                        &candidates,
                        (vw, vh),
                        eye,
                        target,
                        up,
                        glam::Vec2::new(physical.x, physical.y),
                    );
                    self.surface.hover_soul.clone_from(&hit);
                }
            }
        }

        self.interaction.sync(InteractionSnapshot {
            transparent: overlay_transparent,
            click_through_preferred: preferred_click_through,
            chrome_protected: protect_chrome,
            hovering_body: self.surface.hover_soul.is_some(),
            dragging: self.surface.drag.is_some(),
            has_avatar,
            window_visible,
        });
        self.interaction.request_ui(self.overlay_ui_request());

        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        let level = overlay_window_level(protect_chrome, always_on_top);
        overlay.window.set_window_level(level);
        overlay.set_click_through(!self.interaction.cursor_hittest_enabled());
        self.interaction.apply_to_window(&overlay.window);
        let visual = self.scene.visual_geometry();
        let interaction_geom = self.scene.interaction_geometry();
        self.overlay_platform.apply(
            &overlay.window,
            self.interaction.mode(),
            &visual,
            &interaction_geom,
        );
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

    /// Load the active soul once so the Home readiness cards and the companion
    /// list reflect the live companion without first opening the Companion tab
    /// (#1177). The Companion tab re-issues this idempotently.
    fn request_active_soul(&mut self) {
        let soul_id = self.session.soul_id().to_owned();
        if soul_id.is_empty() {
            return;
        }
        let client = Arc::clone(&self.client);
        self.spawn(async move {
            AsyncOutcome::LoadSoul(client.get_soul(&soul_id).await.map_err(|e| e.to_string()))
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
            AsyncOutcome::SendMessage {
                session_id,
                sent_text,
                result,
            } => {
                if session_id != self.session.session_id() {
                    return;
                }
                if let Err(err) = result {
                    self.discard_optimistic_user_row(&sent_text);
                    self.surface.status = err;
                    self.surface.chat_draft = sent_text;
                    self.surface.push_chat_draft = true;
                } else {
                    self.surface.chat_draft.clear();
                    self.surface.push_chat_draft = true;
                    self.surface.streaming_text.clear();
                    self.complete_optimistic_user_row(sent_text);
                    if let Some(chat) = &self.chat {
                        chat.request_redraw();
                    }
                    self.begin_completion_reconciliation();
                }
            }
            AsyncOutcome::DirectReaction { result, .. } => {
                self.direct_reaction_agent_inflight = false;
                if let Err(err) = result {
                    self.surface.status = format!("{}: {err}", i18n::fl("direct-reaction-failed"));
                }
            }
            AsyncOutcome::RetargetSoul { result, .. } => {
                self.direct_reaction_retarget_inflight = false;
                match result {
                    Ok(target) => {
                        self.commit_session_target(target);
                        self.reload_avatar();
                    }
                    Err(err) => {
                        self.surface.status =
                            format!("{}: {err}", i18n::fl("direct-reaction-target-failed"));
                    }
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
            AsyncOutcome::Approval {
                session_id,
                allowed,
                approval_id,
                result,
            } => {
                if session_id != self.session.session_id() {
                    return;
                }
                self.surface.pending_approval = None;
                self.approval_needs_reveal = false;
                match result {
                    Err(err) => self.surface.status = err,
                    Ok(()) => {
                        if allowed {
                            let matched = self
                                .detail
                                .pending_job_retry
                                .as_ref()
                                .is_some_and(|retry| retry.approval_id == approval_id);
                            if matched {
                                let retry = self.detail.pending_job_retry.take();
                                if let Some(retry) = retry {
                                    let client = Arc::clone(&self.client);
                                    self.spawn(async move {
                                        AsyncOutcome::CreateJob(
                                            client
                                                .create_job(&retry.request)
                                                .await
                                                .map_err(|e| e.to_string()),
                                        )
                                    });
                                }
                            }
                        } else {
                            // A deny is the final answer for an armed retry
                            // even when the ask it waited on is gone, so it
                            // can never fire on some later unrelated grant.
                            self.detail.pending_job_retry = None;
                        }
                    }
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
                        let has_assistant = history.messages.iter().any(|m| m.role == "assistant");
                        let has_status = history
                            .messages
                            .iter()
                            .any(|m| m.role == "status" || m.role == "error");
                        let has_terminal = has_assistant || has_status;
                        if !has_terminal
                            && !self.surface.history.messages.is_empty()
                            && self.pending_completion_refreshes > 0
                        {
                            self.session.replace_history(history);
                        } else {
                            self.session.replace_history(history.clone());
                            self.surface.history = history;
                        }
                        if self.pending_completion_refreshes > 0 && has_terminal {
                            self.pending_completion_refreshes = 0;
                        }
                        self.surface.streaming_text.clear();
                        if let Some(chat) = &self.chat {
                            chat.request_redraw();
                        }
                    }
                    Err(err) => self.surface.status = err,
                }
            }
            AsyncOutcome::ReconcileHistory { session_id, result } => {
                if session_id != self.session.session_id() {
                    return;
                }
                match result {
                    Ok(history) => {
                        // Completion requires a newly projected terminal row:
                        // surface seqs come from a sparse event log (hidden
                        // events still consume seqs) and the optimistic user
                        // row carries a synthetic seq, so only an assistant row
                        // beyond the pre-turn assistant proves this turn ended.
                        let progressed = match self.completion_terminal_seq {
                            None => history.messages.iter().any(|m| {
                                m.role == "assistant" || m.role == "status" || m.role == "error"
                            }),
                            Some(terminal) => history.messages.iter().any(|m| {
                                (m.role == "assistant" || m.role == "status" || m.role == "error")
                                    && m.seq > terminal
                            }),
                        };
                        if progressed {
                            self.surface.history = history.clone();
                            self.pending_completion_refreshes = 0;
                        }
                        self.session.replace_history(history);
                    }
                    Err(err) => self.surface.status = err,
                }
                self.completion_reconcile_inflight = false;
                if self.pending_completion_refreshes > 0 {
                    self.schedule_completion_refresh();
                }
            }
            AsyncOutcome::SaveLocalSettings {
                generation,
                result,
                success_status,
            } => {
                if generation != self.local_settings_save_generation {
                    return;
                }
                self.detail.core_status = match result {
                    Ok(()) => success_status.unwrap_or_else(|| i18n::fl("settings-saved")),
                    Err(err) => err,
                };
            }
            AsyncOutcome::LoadCoreSettings(result) => match result {
                Ok(json) => {
                    self.detail.core_settings_text.clone_from(&json);
                    self.detail.core_patch_text.clear();
                    detail::parse_core_fields(&json, &mut self.detail);
                    self.sync_stt_cta_after_settings_parse();
                    self.detail.finish_settings_load();
                    self.surface.chat_setup = self.detail.clone();
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
            AsyncOutcome::LoadMcpCatalog(result) => match result {
                Ok(doc) => {
                    self.detail.mcp_catalog = doc.entries;
                    self.detail.mcp_catalog_source = doc.source;
                    self.detail.mcp_catalog_fallback = doc.fallback;
                }
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::ProbeMcp { generation, result } => {
                if !self.detail.mcp_probe_is_current(generation) {
                    return;
                }
                self.detail.mcp_probe_pending = None;
                self.detail.mcp_probe_result = match result {
                    Ok(response) => Some(response),
                    Err(err) => Some(ene_api::McpProbeResponse {
                        error: Some(err),
                        ..ene_api::McpProbeResponse::default()
                    }),
                };
            }
            AsyncOutcome::SaveMcpCredential { generation, result } => {
                if !self.detail.mcp_probe_is_current(generation) {
                    return;
                }
                self.detail.mcp_credential_draft.inflight = false;
                self.detail.connections_status = match result {
                    Ok(response) => {
                        self.detail.mcp_credential_draft.token.clear();
                        if response.error.is_none() {
                            i18n::fl("connections-mcp-credential-saved")
                        } else {
                            // A rejected token still reached the vault; the
                            // message must say which half succeeded.
                            i18n::fl("connections-mcp-credential-saved-unverified")
                        }
                    }
                    Err(err) => err,
                };
            }
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
            AsyncOutcome::ResolveMemoryFailedKeepCandidate {
                soul_id,
                original,
                result,
            } => {
                if soul_id == self.session.soul_id() {
                    let Err(err) = result else {
                        self.request_memories();
                        return;
                    };
                    self.detail.pending_memories.push(original);
                    self.detail.pending_memories.sort_by(|a, b| a.id.cmp(&b.id));
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
                    self.reload_avatar();
                }
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::ImportCharacter { generation, result } => {
                if !self.detail.activation_is_current(generation) {
                    return;
                }
                self.detail.character_action_pending = false;
                match result {
                    Ok(activated) => {
                        let displayed_before = self.local_settings.displayed_soul_ids.clone();
                        let new_soul_id = activated.character.soul_id.clone();
                        if let Some(target) = activated.target {
                            self.commit_session_target(target);
                        }
                        let display_limited = new_soul_id.as_deref().is_some_and(|id| {
                            self.has_avatar_occupant(id) && !self.include_soul_in_display(id)
                        });
                        let overlay_status = self.reload_avatar();
                        self.detail.invalidate_character();
                        self.detail.core_status = format!(
                            "{}: {}",
                            i18n::fl("character-imported"),
                            activated.character.id
                        );
                        if display_limited {
                            self.detail.core_status.push_str(". ");
                            self.detail
                                .core_status
                                .push_str(&i18n::fl("character-display-full-help"));
                        }
                        if let Some(status) = overlay_status {
                            self.detail.core_status.push_str(". ");
                            self.detail.core_status.push_str(&status);
                        }
                        if self.local_settings.displayed_soul_ids != displayed_before {
                            self.save_local_settings_with_status(Some(
                                self.detail.core_status.clone(),
                            ));
                        }
                        self.request_characters();
                    }
                    Err(err) => {
                        self.surface.status = err.clone();
                        self.detail.core_status = err;
                    }
                }
            }
            AsyncOutcome::ActivateCharacter { generation, result } => {
                if !self.detail.activation_is_current(generation) {
                    return;
                }
                self.detail.character_action_pending = false;
                match result {
                    Ok(activated) => {
                        let displayed_before = self.local_settings.displayed_soul_ids.clone();
                        let new_soul_id = activated.character.soul_id.clone();
                        if let Some(target) = activated.target {
                            self.commit_session_target(target);
                        }
                        let display_limited = new_soul_id.as_deref().is_some_and(|id| {
                            self.has_avatar_occupant(id) && !self.include_soul_in_display(id)
                        });
                        let overlay_status = self.reload_avatar();
                        self.detail.invalidate_character();
                        self.detail.core_status = format!(
                            "{}: {}",
                            i18n::fl("character-activated"),
                            activated.character.id
                        );
                        if display_limited {
                            self.detail.core_status.push_str(". ");
                            self.detail
                                .core_status
                                .push_str(&i18n::fl("character-display-full-help"));
                        }
                        if let Some(status) = overlay_status {
                            self.detail.core_status.push_str(". ");
                            self.detail.core_status.push_str(&status);
                        }
                        if self.local_settings.displayed_soul_ids != displayed_before {
                            self.save_local_settings_with_status(Some(
                                self.detail.core_status.clone(),
                            ));
                        }
                        self.request_characters();
                    }
                    Err(err) => {
                        self.surface.status = err.clone();
                        self.detail.core_status = err;
                    }
                }
            }
            AsyncOutcome::ListCharacters(result) => match result {
                Ok(items) => {
                    self.detail.characters = items;
                    self.detail.character_list_loaded = true;
                }
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
                let submitted = self.detail.submitted_job.take();
                match result {
                    Ok(job) => {
                        self.detail.pending_job_retry = None;
                        self.detail.jobs.retain(|item| item.id != job.id);
                        self.detail.jobs.insert(0, job);
                        self.detail.new_job_title.clear();
                        self.detail.new_job_goal.clear();
                        self.detail.core_status = i18n::fl("jobs-created");
                    }
                    Err(err) => {
                        if create_denied_by_approval(&err) {
                            // The rejection means the goal's delegate.start
                            // ask reached the plane; bind the surfaced ask id
                            // when it is already visible so exactly its Allow
                            // can replay. Late asks match by goal later.
                            let approval_id = self
                                .surface
                                .pending_approval
                                .as_ref()
                                .filter(|ask| {
                                    ask.tool == "delegate.start"
                                        && submitted
                                            .as_ref()
                                            .is_some_and(|job| job.goal == ask.target)
                                })
                                .map(|ask| ask.id.clone());
                            if let Some(request) = submitted {
                                self.detail.pending_job_retry = Some(PendingJobRetry {
                                    approval_id: approval_id.unwrap_or_default(),
                                    request,
                                });
                            }
                        } else {
                            self.detail.pending_job_retry = None;
                        }

                        self.detail.core_status = friendly_create_job_error(&err);
                    }
                }
            }
            AsyncOutcome::CreateSchedule(result) => match result {
                Ok(schedule) => {
                    self.detail.new_schedule_inflight = false;
                    self.detail.schedules.retain(|item| item.id != schedule.id);
                    self.detail.schedules.push(schedule);
                    self.detail.new_schedule_name.clear();
                    self.detail.new_schedule_spec.clear();
                    self.detail.core_status = i18n::fl("schedule-created");
                }
                Err(err) => {
                    self.detail.new_schedule_inflight = false;
                    self.detail.core_status = err;
                }
            },
            AsyncOutcome::CancelJob { result, .. } => match result {
                Ok(()) => self.request_jobs(),
                Err(err) if cancel_already_terminal(&err) => {
                    self.detail.core_status = friendly_cancel_job_error(&err);
                    self.request_jobs();
                }
                Err(err) => self.detail.core_status = err,
            },
            AsyncOutcome::ToggleSchedule { result, .. } => {
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
            AsyncOutcome::LoadTools(result) => match result {
                Ok(items) => self.detail.mcp_tools = items,
                Err(err) => self.detail.connections_status = err,
            },
            AsyncOutcome::ReloadMcpTools(result) => match result {
                Ok(items) => {
                    self.detail.mcp_tools = items;
                    self.detail.core_status = i18n::fl("mcp-reloaded");
                }
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
                    self.detail.stt_plugin_ready = true;
                    if active {
                        // A claimed mic proves a real STT provider is in use,
                        // so any parked Voice-setup CTA is obsolete.
                        self.surface.stt_setup_needed = false;
                    }
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
            AsyncOutcome::ReloadAvatar => {
                self.reload_avatar();
            }
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
                    self.completion_reconcile_inflight = false;
                    self.pending_completion_refreshes = 0;
                    self.completion_terminal_seq = None;
                    self.pending_optimistic_user_rows.clear();
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
                    self.detail.pending_job_retry = None;
                    self.detail.submitted_job = None;
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
        // The create click can be rejected before its ask reaches the stage
        // (event vs HTTP race), so a fresh unbound delegate.start ask binds
        // itself to the stash waiting on the same goal, then surfaces as
        // usual so the user can still resolve it.
        if approval.tool == "delegate.start"
            && let Some(retry) = self.detail.pending_job_retry.as_mut().filter(|retry| {
                retry.approval_id.is_empty() && retry.request.goal == approval.target
            })
        {
            retry.approval_id.clone_from(&approval.id);
        }
        let is_new = self
            .surface
            .pending_approval
            .as_ref()
            .is_none_or(|current| current.id != approval.id);
        self.surface.pending_approval = Some(approval);
        if is_new {
            self.surface.chat_open = true;
            self.approval_needs_reveal = true;
            // A hover hole left armed by a previous interaction keeps the
            // overlay click-through disabled, which lets the WM stack it above
            // the freshly opened chat window.
            self.surface.hover_soul = None;
            self.sync_overlay_interaction();
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

    /// Re-fetch history after a turn completes. The first refresh can still be
    /// stale because the projection lags the end event, so the fetch retries
    /// inside one task until authoritative history shows an assistant row newer
    /// than the last one seen when the arm happened; ordering between send
    /// outcomes and turn-end events does not matter because both entry points
    /// arm through this method. Re-arming keeps the previous terminal marker so
    /// back-to-back turns cannot complete against rows from the predecessor.
    fn begin_completion_reconciliation(&mut self) {
        if !self.completion_reconcile_inflight {
            self.pending_completion_refreshes = MAX_COMPLETION_REFRESHES;
        }
        let terminal = self
            .surface
            .history
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "assistant")
            .map(|m| m.seq)
            .or(self.completion_terminal_seq);
        self.completion_terminal_seq = terminal;
        self.schedule_completion_refresh();
    }

    fn schedule_completion_refresh(&mut self) {
        if self.completion_reconcile_inflight || self.pending_completion_refreshes == 0 {
            return;
        }
        self.pending_completion_refreshes -= 1;
        self.completion_reconcile_inflight = true;
        let session = self.session.clone_handle();
        let session_id = self.session.session_id().to_owned();
        self.spawn(async move {
            AsyncOutcome::ReconcileHistory {
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
        // A mic claim needs a Speech-to-Text provider; without one the claim
        // succeeds but recognition can never run, so surface the Voice setup
        // CTA instead of a silent ON state (#1177).
        if !self.mic_active && !self.detail.stt_plugin_ready {
            self.surface.status = i18n::fl("tray-mic-needs-stt");
            self.surface.stt_setup_needed = true;
            return;
        }
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

    /// A successful mic claim proves STT readiness on its own, but parked
    /// Voice-setup CTAs must also be disarmed as soon as effective settings
    /// show a non-placeholder provider, independent of any mic interaction.
    fn sync_stt_cta_after_settings_parse(&mut self) {
        if self.detail.stt_plugin_ready {
            self.surface.stt_setup_needed = false;
        }
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
        let sent_text = text.clone();
        let mode = self.surface.message_mode;
        self.surface.begin_send();
        self.push_optimistic_user_row(sent_text.clone());
        self.spawn(async move {
            AsyncOutcome::SendMessage {
                session_id,
                sent_text,
                result: session
                    .send(&text, mode)
                    .await
                    .map(|_| ())
                    .map_err(|err| map_turn_err(&err.to_string())),
            }
        });
    }

    /// Paints the user row before the HTTP send resolves; the composer keeps
    /// its editable draft (failure restores it), and a real assistant-era row
    /// from the next refresh supersedes this one.
    fn push_optimistic_user_row(&mut self, text: String) {
        let seq = self.append_optimistic_user_row(text.clone());
        self.pending_optimistic_user_rows
            .push(OptimisticUserRow { seq, text });
    }

    fn complete_optimistic_user_row(&mut self, text: String) {
        let Some(index) = self
            .pending_optimistic_user_rows
            .iter()
            .position(|row| row.text == text)
        else {
            self.append_optimistic_user_row(text);
            return;
        };
        let pending = self.pending_optimistic_user_rows.remove(index);
        let still_visible = self.surface.history.messages.iter().any(|message| {
            message.seq == pending.seq && message.role == "user" && message.text == pending.text
        });
        if !still_visible {
            self.append_optimistic_user_row(pending.text);
        }
    }

    fn discard_optimistic_user_row(&mut self, text: &str) {
        if let Some(index) = self
            .pending_optimistic_user_rows
            .iter()
            .position(|row| row.text == text)
        {
            self.pending_optimistic_user_rows.remove(index);
        }
    }

    fn append_optimistic_user_row(&mut self, text: String) -> u64 {
        let next_seq = self
            .surface
            .history
            .messages
            .last()
            .map_or(0, |m| m.seq + 1);
        self.surface
            .history
            .messages
            .push(ene_api::MessageResponse {
                seq: next_seq,
                role: "user".to_owned(),
                text,
            });
        next_seq
    }

    fn handle_avatar_reaction(&mut self, soul_id: &str, kind: ReactionKind, rate_limited: bool) {
        if !self.local_settings.direct_reactions_enabled {
            return;
        }
        let strength = self.local_settings.direct_reaction_strength;
        let expression = direct_reaction_expression(kind);
        if let Some(overlay) = self.overlay.as_mut()
            && let Some(avatar) = overlay.avatar_mut(soul_id)
        {
            avatar.trigger_interaction_feedback(strength, expression);
        }
        if rate_limited {
            return;
        }
        if self.local_settings.direct_reaction_agent
            && !self.direct_reaction_agent_inflight
            && !self.surface.turn_active
        {
            self.send_direct_reaction(soul_id, kind);
        }
        if self.local_settings.direct_reaction_selects_active
            && soul_id != self.session.soul_id()
            && !self.direct_reaction_retarget_inflight
        {
            self.request_direct_reaction_retarget(soul_id);
        }
    }

    fn send_direct_reaction(&mut self, soul_id: &str, kind: ReactionKind) {
        self.direct_reaction_agent_inflight = true;
        let client = Arc::clone(&self.client);
        let soul_id = soul_id.to_owned();
        let text = direct_reaction_message(kind).to_owned();
        self.spawn(async move {
            let result = send_direct_interaction(&client, &soul_id, &text)
                .await
                .map_err(|err| err.to_string());
            AsyncOutcome::DirectReaction { soul_id, result }
        });
    }

    fn request_direct_reaction_retarget(&mut self, soul_id: &str) {
        self.direct_reaction_retarget_inflight = true;
        let client = Arc::clone(&self.client);
        let soul_id = soul_id.to_owned();
        self.spawn(async move {
            let result = prepare_soul_target(&client, &soul_id)
                .await
                .map_err(|err| err.to_string());
            AsyncOutcome::RetargetSoul { soul_id, result }
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
        let text = question_answer_text(
            &self.surface.question_draft,
            &self.surface.chat_draft,
            &question.prompt,
        );
        self.surface.question_draft.clear();
        self.surface.chat_draft.clear();
        self.surface.push_chat_draft = true;
        let session = self.session.clone_handle();
        let session_id = self.session.session_id().to_owned();
        let sent_text = text.clone();
        self.spawn(async move {
            AsyncOutcome::SendMessage {
                session_id,
                sent_text,
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

    fn respond_approval(&mut self, id: &str, decision: &str) {
        let Some(pending) = self
            .surface
            .pending_approval
            .clone()
            .filter(|pending| pending.id == id)
        else {
            return;
        };
        let session = self.session.clone_handle();
        let session_id = self.session.session_id().to_owned();
        let id = pending.id;
        let decision = decision.to_owned();
        let allowed = decision == "allow" || decision == "allow_and_remember";
        self.spawn(async move {
            AsyncOutcome::Approval {
                session_id,
                allowed,
                approval_id: id.clone(),
                result: session
                    .respond_approval(&id, &decision)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
            }
        });
    }

    fn save_local_settings(&mut self) {
        self.save_local_settings_with_status(None);
    }

    fn save_local_settings_with_status(&mut self, success_status: Option<String>) {
        self.local_settings_save_generation = self.local_settings_save_generation.wrapping_add(1);
        let generation = self.local_settings_save_generation;
        let settings = self.local_settings.clone();
        if settings.mic_device != self.settings.mic_device {
            self.audio.set_mic_device(&settings.mic_device);
        }
        self.settings = settings.clone();
        for (soul_id, pos) in &settings.character_positions {
            self.surface.positions.insert(soul_id.clone(), *pos);
        }
        if let Some(overlay) = self.overlay.as_ref() {
            overlay.window.request_redraw();
        }
        if let Some(detail) = self.detail_win.as_ref() {
            detail.request_redraw();
        }
        i18n::select_language(&settings.language);
        self.sync_chrome_titles();
        if let Some(caption) = &self.caption {
            caption.place_caption(&settings.caption_position);
        }
        self.sync_overlay_interaction();
        self.spawn(async move {
            AsyncOutcome::SaveLocalSettings {
                generation,
                result: save_desktop_settings(&settings).map_err(|err| err.to_string()),
                success_status,
            }
        });
    }

    fn overlay_monitor_target(
        &self,
        monitors: &[MonitorInfo],
    ) -> Option<monitor::ResolvedMonitorTarget> {
        let pointer = crate::platform::global_cursor_position().or_else(|| {
            self.last_global_cursor.map(|cursor| {
                [
                    cursor
                        .x
                        .round()
                        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
                    cursor
                        .y
                        .round()
                        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
                ]
            })
        });
        let pointer_fallback_id = self
            .detail_win
            .as_ref()
            .and_then(|window| window.window.current_monitor())
            .or_else(|| {
                self.overlay
                    .as_ref()
                    .and_then(|window| window.window.current_monitor())
            })
            .map(|handle| monitor::stable_id(&handle));
        monitor::resolve_target(
            monitors,
            OverlayMonitorMode::from_setting(&self.local_settings.overlay_monitor_mode),
            &self.local_settings.overlay_monitor_id,
            &self.local_settings.overlay_monitor_name,
            self.local_settings.overlay_monitor_position,
            pointer,
            pointer_fallback_id.as_deref(),
        )
    }

    fn record_overlay_monitor(&mut self, target: &monitor::ResolvedMonitorTarget) {
        let mode = OverlayMonitorMode::from_setting(&self.local_settings.overlay_monitor_mode);
        if mode != OverlayMonitorMode::Selected || target.fallback {
            return;
        }
        let Some(monitor) = target.monitor.as_ref() else {
            return;
        };
        let id = monitor.id.clone();
        let name = monitor.name.clone().unwrap_or_default();
        let position = monitor.position;
        let size = monitor.size;
        let scale_factor = monitor.scale_factor;
        let settings = &mut self.local_settings;
        settings.overlay_monitor_id = id;
        settings.overlay_monitor_name = name;
        settings.overlay_monitor_position = position;
        settings.overlay_monitor_size = size;
        settings.overlay_monitor_scale_factor = scale_factor;
    }

    fn clamp_overlay_positions(&mut self) -> bool {
        let mut changed = false;
        for (soul_id, position) in &mut self.surface.positions {
            let clamped = crate::drag::clamp_position(*position);
            if position_changed(clamped, *position) {
                *position = clamped;
                changed = true;
            }
            self.local_settings
                .character_positions
                .insert(soul_id.clone(), clamped);
        }
        for position in self.local_settings.character_positions.values_mut() {
            let clamped = crate::drag::clamp_position(*position);
            if position_changed(clamped, *position) {
                *position = clamped;
                changed = true;
            }
        }
        let active_soul = self.session.soul_id().to_owned();
        let before = [
            self.local_settings.character_x,
            self.local_settings.character_y,
        ];
        crate::settings::mirror_active_position(&mut self.local_settings, &active_soul);
        changed
            || position_changed(
                before,
                [
                    self.local_settings.character_x,
                    self.local_settings.character_y,
                ],
            )
    }

    fn apply_overlay_monitor(&mut self, event_loop: &ActiveEventLoop, fit_positions: bool) {
        let monitors = monitor::inventory(event_loop);
        self.monitors.clone_from(&monitors);
        let Some(target) = self.overlay_monitor_target(&monitors) else {
            self.detail.overlay_monitor_notice = i18n::fl("settings-overlay-no-monitors");
            return;
        };
        let target_position =
            PhysicalPosition::new(target.rect.position[0], target.rect.position[1]);
        let target_size = PhysicalSize::new(target.rect.size[0], target.rect.size[1]);
        let gpu = self.gpu.as_ref();
        if let Some(overlay) = self.overlay.as_mut() {
            if overlay.window.outer_position().ok() != Some(target_position) {
                overlay.window.set_outer_position(target_position);
            }
            let current_size = overlay.window.inner_size();
            let applied_size = if current_size == target_size {
                current_size
            } else {
                overlay
                    .window
                    .request_inner_size(target_size)
                    .unwrap_or(current_size)
            };
            if let Some(gpu) = gpu {
                overlay.resize(gpu, applied_size);
            }
            overlay.window.request_redraw();
        }
        self.record_overlay_monitor(&target);
        if fit_positions || target.fallback {
            self.clamp_overlay_positions();
        }
        if target.fallback {
            self.detail.overlay_monitor_notice = i18n::fl("settings-overlay-monitor-fallback");
        } else if let Some(monitor) = target.monitor.as_ref() {
            let number = (monitor.ordinal + 1).to_string();
            let name = monitor.name.clone().unwrap_or_else(|| {
                i18n::format("settings-overlay-display", &[("number", number.as_str())])
            });
            let size = format!("{}×{}", monitor.size[0], monitor.size[1]);
            let scale = format!("{:.0}%", monitor.scale_factor * 100.0);
            self.detail.overlay_monitor_notice = i18n::format(
                "settings-overlay-monitor-moved",
                &[
                    ("monitor", name.as_str()),
                    ("size", size.as_str()),
                    ("scale", scale.as_str()),
                ],
            );
        } else {
            self.detail.overlay_monitor_notice = i18n::fl("settings-overlay-all-moved");
        }
        self.save_local_settings();
    }

    fn refresh_monitor_inventory(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if now < self.next_monitor_probe {
            return;
        }
        self.next_monitor_probe = now + Duration::from_millis(500);
        let monitors = monitor::inventory(event_loop);
        let changed = monitors != self.monitors;
        self.monitors = monitors;
        if changed && self.overlay.is_some() {
            self.apply_overlay_monitor(event_loop, true);
        }
    }

    fn process_overlay_monitor_action(&mut self, event_loop: &ActiveEventLoop) {
        if !std::mem::take(&mut self.detail.overlay_monitor_apply_pending) {
            return;
        }
        let fit_positions = std::mem::take(&mut self.detail.overlay_monitor_fit_pending);
        self.detail.save_local_pending = false;
        self.apply_overlay_monitor(event_loop, fit_positions);
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
            let _ = tray;
        }
        for command in tray_commands {
            self.dispatch_shell_command(event_loop, command);
        }
        let hotkey_command = self.hotkeys.as_mut().and_then(HotkeyManager::poll);
        if let Some(command) = hotkey_command {
            self.dispatch_shell_command(event_loop, command);
        }
        self.surface.spotlight_hotkey_ok = self
            .hotkeys
            .as_ref()
            .is_some_and(HotkeyManager::spotlight_active);
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
                SurfaceAction::SelectCompanion { soul_id } => self.select_occupant(&soul_id),
                SurfaceAction::SelectGreeting { index } => self.select_greeting(index),
                SurfaceAction::BargeIn => self.barge_in(),
                SurfaceAction::CancelTurn => self.cancel_turn(),
                SurfaceAction::ToggleMic => self.toggle_mic(),
                SurfaceAction::Approval { id, decision } => self.respond_approval(&id, &decision),
                SurfaceAction::AnswerQuestion => self.answer_question(),
                SurfaceAction::OpenDetail(tab) => self.open_detail(event_loop, tab),
                SurfaceAction::Quit => self.surface.quit = true,
                SurfaceAction::PersistBodyPosition { soul_id } => {
                    self.persist_body_position(&soul_id);
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

    /// Seed an empty surface from the boot-time session snapshot. A turn that
    /// completed before the first paint leaves that snapshot empty until its
    /// refresh lands; seeding stays one-shot so a later refresh always owns
    /// the surface rows instead of being erased by this copy.
    fn seed_history_from_session(&mut self) {
        let cached = self.session.history();
        if !cached.messages.is_empty() {
            self.surface.history = cached;
        }
    }

    /// Stores one body overlay position in the settings map. The active soul
    /// coordinates are also mirrored into the legacy scalar keys so
    /// hand-edited config files stay readable; reads still prefer the map.
    fn persist_body_position(&mut self, soul_id: &str) {
        let pos = self.surface.positions.get(soul_id).copied();
        if let Some(pos) = pos {
            self.local_settings
                .character_positions
                .insert(soul_id.to_owned(), pos);
            if soul_id == self.session.soul_id() {
                self.local_settings.character_x = pos[0];
                self.local_settings.character_y = pos[1];
            }
        }
        self.save_local_settings();
    }

    fn has_avatar_occupant(&self, soul_id: &str) -> bool {
        self.session.occupants().iter().any(|occupant| {
            occupant.soul_id == soul_id && crate::core::session::occupant_has_avatar(occupant)
        })
    }

    fn visible_display_count(&self) -> usize {
        self.local_settings
            .displayed_soul_ids
            .iter()
            .filter(|soul_id| {
                !self.temporarily_hidden_souls.contains(*soul_id)
                    && self.has_avatar_occupant(soul_id)
            })
            .count()
    }

    fn include_soul_in_display(&mut self, soul_id: &str) -> bool {
        if !self.has_avatar_occupant(soul_id) {
            return false;
        }
        if self
            .local_settings
            .displayed_soul_ids
            .iter()
            .any(|current| current == soul_id)
        {
            self.temporarily_hidden_souls.remove(soul_id);
            return true;
        }
        if self.visible_display_count() >= crate::core::session::MAX_OVERLAY_BODIES {
            return false;
        }
        self.local_settings
            .displayed_soul_ids
            .push(soul_id.to_owned());
        self.local_settings.displayed_souls_initialized = true;
        true
    }

    fn apply_display_action(&mut self, action: DisplayAction) {
        match action {
            DisplayAction::Show(soul_id) => {
                if !self.has_avatar_occupant(&soul_id) {
                    self.detail.core_status = i18n::fl("character-text-only-reason");
                    return;
                }
                if !self.include_soul_in_display(&soul_id) {
                    let capacity = crate::core::session::MAX_OVERLAY_BODIES.to_string();
                    self.detail.core_status =
                        i18n::format("character-display-limit", &[("capacity", &capacity)]);
                    return;
                }
                self.detail.core_status = match self.reload_avatar() {
                    Some(status) => status,
                    None => i18n::fl("character-display-updated"),
                };
                self.save_local_settings_with_status(Some(self.detail.core_status.clone()));
                self.request_detail_and_overlay_redraw();
            }
            DisplayAction::TemporarilyHide(soul_id) => {
                if self
                    .local_settings
                    .displayed_soul_ids
                    .iter()
                    .any(|current| current == &soul_id)
                {
                    self.temporarily_hidden_souls.insert(soul_id);
                    self.reload_avatar();
                    self.detail.core_status = i18n::fl("character-temporarily-hidden-status");
                    self.request_detail_and_overlay_redraw();
                }
            }
            DisplayAction::Remove(soul_id) => {
                let before = self.local_settings.displayed_soul_ids.len();
                self.local_settings
                    .displayed_soul_ids
                    .retain(|current| current != &soul_id);
                self.temporarily_hidden_souls.remove(&soul_id);
                if self.local_settings.displayed_soul_ids.len() != before {
                    self.local_settings.displayed_souls_initialized = true;
                    self.reload_avatar();
                    let status = i18n::fl("character-removed-from-display");
                    self.save_local_settings_with_status(Some(status.clone()));
                    self.detail.core_status = status;
                    self.request_detail_and_overlay_redraw();
                }
            }
            DisplayAction::MoveUp(soul_id) => {
                if let Some(index) = self
                    .local_settings
                    .displayed_soul_ids
                    .iter()
                    .position(|current| current == &soul_id)
                    && index > 0
                {
                    self.local_settings
                        .displayed_soul_ids
                        .swap(index, index - 1);
                    self.reload_avatar();
                    let status = i18n::fl("character-display-updated");
                    self.save_local_settings_with_status(Some(status.clone()));
                    self.detail.core_status = status;
                    self.request_detail_and_overlay_redraw();
                }
            }
            DisplayAction::MoveDown(soul_id) => {
                if let Some(index) = self
                    .local_settings
                    .displayed_soul_ids
                    .iter()
                    .position(|current| current == &soul_id)
                    && index + 1 < self.local_settings.displayed_soul_ids.len()
                {
                    self.local_settings
                        .displayed_soul_ids
                        .swap(index, index + 1);
                    self.reload_avatar();
                    let status = i18n::fl("character-display-updated");
                    self.save_local_settings_with_status(Some(status.clone()));
                    self.detail.core_status = status;
                    self.request_detail_and_overlay_redraw();
                }
            }
        }
    }

    fn request_detail_and_overlay_redraw(&self) {
        if let Some(overlay) = self.overlay.as_ref() {
            overlay.window.request_redraw();
        }
        if let Some(detail) = self.detail_win.as_ref() {
            detail.request_redraw();
        }
        if let Some(chat) = self.chat.as_ref() {
            chat.request_redraw();
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
                    self.begin_completion_reconciliation();
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
            LiveEvent::SessionNote { text } => {
                if !text.is_empty() {
                    tracing::info!(%text, "tool denied by approval");
                    self.detail.push_log(crate::detail::LogKind::Tool, text);
                }
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
                soul_id,
                expression,
                ..
            } => {
                if abort {
                    self.abort_audio_playback();
                } else {
                    let played = match self.audio.play_pcm(&pcm, sample_rate) {
                        Ok(()) => true,
                        Err(err) => {
                            tracing::debug!(error = %err, "audio playback failed");
                            false
                        }
                    };
                    if played && let Some(label) = expression.as_deref() {
                        self.apply_expression_cue(soul_id.as_deref(), label);
                    }
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

    fn reload_avatar(&mut self) -> Option<String> {
        self.surface.hover_soul = None;
        self.surface.drag = None;
        self.gesture.cancel_all();
        self.interaction.cancel(CancelReason::AvatarsChanged);
        let all_specs = self.session.avatar_loads();
        let available_soul_ids: Vec<String> =
            all_specs.iter().map(|spec| spec.soul_id.clone()).collect();
        if normalize_displayed_souls(
            &mut self.local_settings.displayed_soul_ids,
            &mut self.local_settings.displayed_souls_initialized,
            &available_soul_ids,
        ) {
            self.save_local_settings();
        }
        self.temporarily_hidden_souls.retain(|soul_id| {
            available_soul_ids
                .iter()
                .any(|available| available == soul_id)
        });
        let selected_soul_ids = ordered_visible_souls(
            &self.local_settings.displayed_soul_ids,
            &self.temporarily_hidden_souls,
            crate::core::session::MAX_OVERLAY_BODIES,
        );
        let specs: Vec<AvatarLoad> = selected_soul_ids
            .iter()
            .filter_map(|soul_id| {
                all_specs
                    .iter()
                    .find(|spec| spec.soul_id == *soul_id)
                    .map(|spec| AvatarLoad {
                        soul_id: spec.soul_id.clone(),
                        path: spec.path.clone(),
                        motions_dir: spec.motions_dir.clone(),
                    })
            })
            .collect();
        let selection_exceeds_capacity =
            self.local_settings.displayed_soul_ids.len() > crate::core::session::MAX_OVERLAY_BODIES;
        let gpu = self.gpu.as_ref()?;
        let overlay = self.overlay.as_mut()?;
        if specs.is_empty() {
            overlay.clear_avatars();
            let status = if all_specs.is_empty() {
                i18n::fl("overlay-no-avatar")
            } else {
                i18n::fl("character-display-empty-help")
            };
            self.surface.status.clone_from(&status);
            return Some(status);
        }
        match overlay.load_avatars(gpu, &specs) {
            Ok(report) => {
                let active_soul = self.session.soul_id().to_owned();
                let soul_ids: Vec<String> = specs.iter().map(|spec| spec.soul_id.clone()).collect();
                let saved = &self.local_settings.character_positions;
                let valid: std::collections::HashSet<&str> =
                    soul_ids.iter().map(std::string::String::as_str).collect();
                self.surface.positions.extend(
                    saved
                        .iter()
                        .filter(|(k, _)| valid.contains(k.as_str()))
                        .map(|(k, v)| (k.clone(), *v)),
                );
                let legacy_pos = [self.settings.character_x, self.settings.character_y];
                crate::settings::seed_character_positions(
                    &mut self.surface.positions,
                    &soul_ids,
                    &active_soul,
                    legacy_pos,
                );
                let capacity_status = selection_exceeds_capacity.then(|| {
                    let capacity = crate::core::session::MAX_OVERLAY_BODIES.to_string();
                    i18n::format("character-display-limit", &[("capacity", &capacity)])
                });
                let failure_status = (!report.failures.is_empty()).then(|| {
                    let failures = report
                        .failures
                        .iter()
                        .map(|failure| format!("{} ({})", failure.soul_id, failure.error))
                        .collect::<Vec<_>>()
                        .join(", ");
                    i18n::format("character-overlay-partial-load", &[("failures", &failures)])
                });
                let status = match (failure_status, capacity_status) {
                    (Some(failure), Some(capacity)) => format!("{failure}; {capacity}"),
                    (Some(failure), None) => failure,
                    (None, Some(capacity)) => capacity,
                    (None, None) => i18n::fl("status-ready"),
                };
                self.surface.status.clone_from(&status);
                tracing::info!(count = report.loaded, "loaded overlay VRM bodies");
                (status != i18n::fl("status-ready")).then_some(status)
            }
            Err(err) => {
                let status = format!("{}: {err}", i18n::fl("character-overlay-load-failed"));
                self.surface.status.clone_from(&status);
                tracing::warn!(error = %err, "VRM load failed");
                Some(status)
            }
        }
    }

    fn cycle_occupant(&mut self, delta: i32) {
        let soul_id = crate::core::session::next_avatar_occupant(
            self.session.occupants(),
            self.session.soul_id(),
            delta,
        )
        .map(|occupant| occupant.soul_id.clone());
        let Some(soul_id) = soul_id else {
            self.surface.status = i18n::fl("overlay-no-avatar");
            return;
        };
        self.select_occupant(&soul_id);
    }

    fn active_companion_label(&self) -> String {
        self.companion_label_for_soul(self.session.soul_id())
    }

    fn chat_targets(&self) -> Vec<surface::ChatTarget> {
        self.session
            .occupants()
            .iter()
            .map(|occupant| surface::ChatTarget {
                soul_id: occupant.soul_id.clone(),
                label: self.companion_label_for_soul(&occupant.soul_id),
                active: occupant.soul_id == self.session.soul_id(),
            })
            .collect()
    }

    fn companion_label_for_soul(&self, soul_id: &str) -> String {
        disambiguated_occupant_label(self.session.occupants(), soul_id)
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "motion picker remains on DetailUiState until the Slint companion tab grows a combo"
        )
    )]
    fn overlay_motion_controls(&self) -> Vec<detail::MotionControl> {
        self.overlay
            .as_ref()
            .map(|overlay| {
                overlay
                    .slots
                    .iter()
                    .map(|slot| detail::MotionControl {
                        soul_id: slot.soul_id.clone(),
                        label: self.companion_label_for_soul(&slot.soul_id),
                        current: slot.avatar.current_motion().map(ToOwned::to_owned),
                        manual_override: slot.avatar.motion_is_manually_overridden(),
                        names: slot
                            .avatar
                            .motion_names()
                            .into_iter()
                            .map(ToOwned::to_owned)
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn apply_motion_command(&mut self, command: detail::MotionCommand) {
        match command {
            detail::MotionCommand::SelectSoul(soul_id) => self.select_occupant(&soul_id),
            detail::MotionCommand::SelectMotion { soul_id, name } => {
                let selected = self
                    .overlay
                    .as_mut()
                    .and_then(|overlay| overlay.avatar_mut(&soul_id))
                    .is_some_and(|avatar| avatar.select_motion_manually(&name));
                self.surface.status = if selected {
                    format!("{}: {name}", i18n::fl("motion-selected"))
                } else {
                    i18n::fl("motion-selection-failed")
                };
            }
            detail::MotionCommand::Stop { soul_id } => {
                if let Some(avatar) = self
                    .overlay
                    .as_mut()
                    .and_then(|overlay| overlay.avatar_mut(&soul_id))
                {
                    avatar.stop_motion();
                    self.surface.status = i18n::fl("motion-stopped");
                }
            }
            detail::MotionCommand::Reset { soul_id } => {
                if let Some(avatar) = self
                    .overlay
                    .as_mut()
                    .and_then(|overlay| overlay.avatar_mut(&soul_id))
                {
                    avatar.reset_motion();
                    self.surface.status = i18n::fl("motion-reset-done");
                }
            }
        }
    }

    fn cycle_active_motion(&mut self, delta: i32) {
        let soul_id = self.session.soul_id().to_owned();
        let status = if let Some(overlay) = self.overlay.as_mut() {
            if let Some(avatar) = overlay.avatar_or_first_mut(&soul_id) {
                let has_motions = !avatar.motion_names().is_empty();
                avatar.cycle_motion(delta);
                if let Some(name) = avatar.current_motion() {
                    format!("{}: {name}", i18n::fl("motion-changed"))
                } else if has_motions {
                    i18n::fl("motion-selection-failed")
                } else {
                    i18n::fl("motion-no-motions")
                }
            } else {
                i18n::fl("motion-no-avatars")
            }
        } else {
            i18n::fl("motion-no-avatars")
        };
        self.surface.status = status;
    }

    fn select_occupant(&mut self, soul_id: &str) {
        if soul_id == self.session.soul_id() {
            return;
        }
        if !self
            .session
            .occupants()
            .iter()
            .any(|occupant| occupant.soul_id == soul_id)
        {
            self.surface.status = i18n::fl("overlay-no-avatar");
            return;
        }
        let label = self.companion_label_for_soul(soul_id);
        if let Err(err) = self.runtime.block_on(self.session.retarget_soul(soul_id)) {
            self.surface.status = err.to_string();
            return;
        }
        self.feeds = spawn_event_feeds(&self.rt_handle, &self.client, self.session.session_id());
        self.pending_optimistic_user_rows.clear();
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
        self.completion_reconcile_inflight = false;
        self.pending_completion_refreshes = 0;
        self.completion_terminal_seq = None;
        self.pending_optimistic_user_rows.clear();
        self.surface.history = self.session.history();
        self.surface.greetings = self.session.greetings().to_vec();
        self.surface.greeting_inflight = false;
        self.surface.greeting_status.clear();
        self.surface.streaming_text.clear();
        self.surface.turn_active = false;
        self.surface.pending_approval = None;
        self.approval_needs_reveal = false;
        self.surface.pending_question = None;
        self.detail.pending_job_retry = None;
        self.detail.submitted_job = None;
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
                self.cycle_active_motion(-1);
            }
            Key::Character(ch) if ch.as_str().eq_ignore_ascii_case("s") => {
                self.cycle_active_motion(1);
            }
            _ => {}
        }
    }

    fn abort_audio_playback(&mut self) {
        self.audio.stop();
        self.viseme.reset();
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.reset_visemes();
            for slot in &mut overlay.slots {
                slot.avatar.clear_expression_cue();
            }
        }
    }

    fn apply_expression_cue(&mut self, soul_id: Option<&str>, label: &str) {
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        let session_soul = self.session.soul_id().to_owned();
        let avatar = match soul_id {
            Some(soul) => overlay.avatar_mut(soul),
            None => overlay.avatar_or_first_mut(&session_soul),
        };
        if let Some(avatar) = avatar {
            avatar.apply_expression_cue(label);
        }
    }

    fn camera_basis(&self) -> Option<(glam::Vec3, glam::Vec3, glam::Vec3, u32, u32)> {
        let overlay = self.overlay.as_ref()?;
        let avatar = overlay.first_avatar()?;
        let cam = avatar.camera();
        let size = overlay.window.inner_size();
        Some((
            glam::Vec3::from(cam.eye()),
            glam::Vec3::from(cam.target()),
            glam::Vec3::from(ene_vrm::camera::DEFAULT_UP),
            size.width.max(1),
            size.height.max(1),
        ))
    }
    fn overlay_hit_candidates(&self) -> Vec<crate::drag::HitCandidate> {
        self.overlay
            .as_ref()
            .map(|overlay| {
                overlay
                    .slots
                    .iter()
                    .map(|slot| {
                        let (min, max) = slot.avatar.world_aabb();
                        crate::drag::HitCandidate {
                            soul_id: slot.soul_id.clone(),
                            world_center: (min + max) / 2.0,
                            aabb_min: min,
                            aabb_max: max,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn on_overlay_cursor_moved(&mut self, position: winit::dpi::PhysicalPosition<f64>) {
        self.on_overlay_pointer_moved(PointerKind::Mouse, 0, position);
    }

    fn on_overlay_pointer_moved(
        &mut self,
        pointer: PointerKind,
        id: u64,
        position: winit::dpi::PhysicalPosition<f64>,
    ) {
        if self.overlay.is_none() {
            return;
        }
        let physical = position.cast::<f32>();
        self.last_cursor = Some(physical);

        let Some((eye, target, up, vw, vh)) = self.camera_basis() else {
            return;
        };
        let cursor = glam::Vec2::new(physical.x, physical.y);
        let candidates = self.overlay_hit_candidates();
        let hit_soul = crate::drag::hit_test(&candidates, (vw, vh), eye, target, up, cursor);
        self.surface.hover_soul.clone_from(&hit_soul);

        let cursor_world =
            crate::drag::cursor_logical_to_world_2d(cursor, (vw, vh), eye, target, up);
        if self.gesture.move_to(pointer, id, cursor).is_dragging() {
            crate::drag::drag_body(
                &mut self.surface.drag,
                &mut self.surface.positions,
                cursor_world,
            );
        }

        self.sync_overlay_interaction();
        if let Some(overlay) = self.overlay.as_ref() {
            overlay.window.request_redraw();
        }
    }

    #[expect(dead_code, reason = "kept for symmetry with on_overlay_release")]
    fn on_overlay_press(&mut self) {
        self.on_overlay_pointer_press_with_protection(PointerKind::Mouse, 0, None);
    }

    fn on_overlay_pointer_press_with_protection(
        &mut self,
        pointer: PointerKind,
        id: u64,
        position: Option<winit::dpi::PhysicalPosition<f64>>,
    ) {
        // When chrome has focus, still allow a stationary body hit to trigger
        // a reaction; otherwise a focused Detail/Chat would swallow all clicks.
        if self.overlay_focus.protects() {
            let pos = position.or_else(|| {
                self.last_cursor.map(|logical| {
                    let Some(overlay) = self.overlay.as_ref() else {
                        return winit::dpi::PhysicalPosition::new(0.0, 0.0);
                    };
                    let scale = overlay.window.scale_factor() as f32;
                    winit::dpi::PhysicalPosition::new(
                        f64::from(logical.x) * f64::from(scale),
                        f64::from(logical.y) * f64::from(scale),
                    )
                })
            });
            if let Some(physical) = pos {
                let Some(overlay) = self.overlay.as_ref() else {
                    return;
                };
                let logical = physical.to_logical::<f32>(overlay.window.scale_factor());
                if let Some((eye, target, up, vw, vh)) = self.camera_basis() {
                    let cursor = glam::Vec2::new(logical.x, logical.y);
                    let candidates = self.overlay_hit_candidates();
                    if crate::drag::hit_test(&candidates, (vw, vh), eye, target, up, cursor)
                        .is_none()
                    {
                        return;
                    }
                } else {
                    return;
                }
            } else {
                return;
            }
        }
        self.on_overlay_pointer_press(pointer, id, position);
    }

    fn on_overlay_pointer_press(
        &mut self,
        pointer: PointerKind,
        id: u64,
        position: Option<winit::dpi::PhysicalPosition<f64>>,
    ) {
        if let Some(position) = position {
            if self.overlay.is_none() {
                return;
            }
            self.last_cursor = Some(position.cast::<f32>());
        }
        let Some(cursor_physical) = self.last_cursor else {
            return;
        };
        let Some((eye, target, up, vw, vh)) = self.camera_basis() else {
            return;
        };
        let cursor = glam::Vec2::new(cursor_physical.x, cursor_physical.y);
        if matches!(self.scene.hit(cursor), SceneHit::Ui { .. }) {
            return;
        }
        let candidates = self.overlay_hit_candidates();
        let Some(soul_id) = crate::drag::hit_test(&candidates, (vw, vh), eye, target, up, cursor)
        else {
            self.surface.drag = None;
            self.gesture.cancel(pointer, id);
            self.sync_overlay_interaction();
            return;
        };

        let stored = self.surface.positions.get(&soul_id).copied();
        let cursor_world =
            crate::drag::cursor_logical_to_world_2d(cursor, (vw, vh), eye, target, up);
        if self
            .gesture
            .press(pointer, id, cursor, Some(&soul_id), Instant::now())
        {
            crate::drag::press_body(&mut self.surface.drag, Some(&soul_id), stored, cursor_world);
        }
        self.sync_overlay_interaction();
    }

    fn on_overlay_release(&mut self) {
        self.on_overlay_pointer_release(PointerKind::Mouse, 0);
    }

    fn on_overlay_pointer_release(&mut self, pointer: PointerKind, id: u64) {
        let end = self.gesture.release(pointer, id, Instant::now());
        match end {
            EndResult::Dragged { soul_id } => {
                let _ = crate::drag::release_body(&mut self.surface.drag);
                self.surface
                    .push_action(SurfaceAction::PersistBodyPosition { soul_id });
            }
            EndResult::Reaction {
                soul_id,
                kind,
                rate_limited,
            } => {
                let _ = crate::drag::release_body(&mut self.surface.drag);
                self.select_occupant(&soul_id);
                self.handle_avatar_reaction(&soul_id, kind, rate_limited);
            }
            EndResult::None => {}
        }
        self.sync_overlay_interaction();
    }

    fn cancel_overlay_pointer(&mut self, pointer: PointerKind, id: u64) {
        if self.gesture.cancel(pointer, id) {
            self.surface.drag = None;
            self.sync_overlay_interaction();
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
        let look_at_active =
            self.interaction.cursor_hittest_enabled() && self.local_settings.look_at_strength > 0.0;
        let look = if look_at_active {
            if let (Some(cursor), Some((size, eye, target, head))) = (cursor, look_input) {
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
            }
        } else {
            None
        };
        let soul = self.session.soul_id().to_owned();
        let drag_soul = self
            .surface
            .drag
            .as_ref()
            .map(|drag| drag.soul_id().to_owned());
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.set_interaction_targets(
                Some(soul.as_str()),
                self.surface.hover_soul.as_deref(),
                drag_soul.as_deref(),
            );
        }
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let surface_positions = self.surface.positions.clone();
        let highlight_soul = overlay_outline_soul(
            self.surface
                .drag
                .as_ref()
                .map(crate::drag::BodyDrag::soul_id),
            self.surface.hover_soul.as_deref(),
        )
        .map(str::to_owned);
        let mut corrected_positions = Vec::new();
        {
            let Some(overlay) = self.overlay.as_mut() else {
                return;
            };
            let size = overlay.window.inner_size();
            let viewport = (size.width.max(1), size.height.max(1));
            for slot in &mut overlay.slots {
                slot.avatar.model_scale =
                    crate::settings::effective_model_scale(&self.local_settings, &slot.soul_id);
                let pos = surface_positions.get(&slot.soul_id).copied().unwrap_or(
                    crate::settings::default_position_for(&slot.soul_id, soul.as_str()),
                );
                let world = crate::drag::normalized_to_world(pos);
                let fitted = slot
                    .avatar
                    .fit_world_offset([world[0], world[1], 0.0], viewport);
                slot.avatar.world_offset = fitted;
                let fitted_pos =
                    crate::drag::world_to_normalized(glam::Vec2::new(fitted[0], fitted[1]));
                if (fitted_pos[0] - pos[0]).abs() > 0.0001
                    || (fitted_pos[1] - pos[1]).abs() > 0.0001
                {
                    corrected_positions.push((slot.soul_id.clone(), fitted_pos));
                }
                slot.avatar.tick_expression_cue(dt);
            }
            match overlay.tick_and_render(
                gpu,
                look,
                Some(visemes),
                Some(soul.as_str()),
                highlight_soul.as_deref(),
            ) {
                Ok(drew) => {
                    if drew {
                        overlay.window.request_redraw();
                    }
                }
                Err(err) => match err {
                    OverlayError::Surface(_) => {
                        tracing::debug!(error = %err, "overlay surface skipped");
                    }
                    OverlayError::Avatar(inner) => {
                        tracing::debug!(error = %inner, "overlay avatar");
                    }
                },
            }
        }
        let layout_changed = !corrected_positions.is_empty();
        for (soul_id, position) in corrected_positions {
            self.surface.positions.insert(soul_id.clone(), position);
            self.local_settings
                .character_positions
                .insert(soul_id.clone(), position);
            if soul_id == soul {
                self.local_settings.character_x = position[0];
                self.local_settings.character_y = position[1];
            }
        }
        if layout_changed {
            self.save_local_settings();
        }
        self.rebuild_scene();
        self.sync_overlay_slint();
    }

    fn sync_overlay_slint(&mut self) {
        let caption = self.surface.caption.clone();
        let speaker = self.active_companion_label();
        let head = self
            .scene
            .anchors()
            .iter()
            .find(|anchor| anchor.kind == StageAnchorKind::Head)
            .map(|anchor| anchor.position);
        let notice = self.surface.overlay_notice.clone();
        let greetings: Vec<(String, String)> = self
            .surface
            .greetings
            .iter()
            .map(|g| {
                (
                    g.index.to_string(),
                    g.text.lines().next().unwrap_or("").to_owned(),
                )
            })
            .collect();
        let key = OverlaySlintKey {
            caption: caption.clone(),
            speaker: speaker.clone(),
            notice: notice.clone(),
            greetings: greetings.clone(),
            head: head.map(|position| (snap_px(position.x), snap_px(position.y))),
        };
        let multiple_greetings = greetings.len() > 1;
        let skip_setters = self.last_overlay_slint.as_ref() == Some(&key);
        let actions = {
            let Some(overlay) = self.overlay.as_mut() else {
                return;
            };
            let Some(layer) = overlay.slint_layer_mut() else {
                return;
            };
            layer.ensure_component();
            if !skip_setters {
                if let Some(ui) = layer.component() {
                    let visible = !caption.is_empty();
                    ui.set_bubble_text(caption.into());
                    ui.set_bubble_speaker(speaker.into());
                    ui.set_bubble_visible(visible);
                    ui.set_bubble_clickable(false);
                    if let Some(position) = head {
                        ui.set_bubble_x(position.x);
                        ui.set_bubble_y((position.y - 72.0).max(8.0));
                    }
                    ui.set_status_visible(!notice.is_empty());
                    ui.set_status_text(notice.into());
                    apply_theme_overlay(ui, &self.local_settings.theme);
                }
                if multiple_greetings {
                    let refs: Vec<(&str, &str)> = greetings
                        .iter()
                        .map(|(id, label)| (id.as_str(), label.as_str()))
                        .collect();
                    layer.set_choices(&refs);
                } else {
                    layer.set_choices(&[]);
                }
            }
            layer.take_actions()
        };
        if !skip_setters {
            self.last_overlay_slint = Some(key);
        }
        for action in actions {
            match action {
                OverlayUiAction::Choice(id) => {
                    if let Ok(index) = id.parse::<u32>() {
                        self.surface
                            .push_action(SurfaceAction::SelectGreeting { index });
                    }
                }
                OverlayUiAction::Bubble => {}
            }
        }
    }

    fn overlay_ui_request(&self) -> UiInteractionRequest {
        let (clickable, focusable) = self.scene.overlay_ui_flags();
        if focusable {
            UiInteractionRequest::Focus
        } else if clickable {
            UiInteractionRequest::Interactive
        } else {
            UiInteractionRequest::None
        }
    }

    fn rebuild_scene(&mut self) {
        let Some(overlay) = self.overlay.as_ref() else {
            self.scene.set_visuals(Vec::new());
            self.scene.set_interactions(Vec::new());
            return;
        };
        let size = overlay.window.inner_size();
        let viewport = (size.width.max(1), size.height.max(1));
        let mut visuals = Vec::new();
        let mut interactions = Vec::new();
        let mut anchors = Vec::new();
        const PARTS: [AvatarPart; 5] = [
            AvatarPart::Body,
            AvatarPart::Head,
            AvatarPart::Torso,
            AvatarPart::LeftHand,
            AvatarPart::RightHand,
        ];
        for slot in &overlay.slots {
            let view_proj = slot.avatar.camera().debug_proj() * slot.avatar.camera().debug_view();
            let (min, max) = slot.avatar.world_aabb();
            let body = world_aabb_to_px(min, max, view_proj, viewport);
            visuals.push(VisualPrimitive::AvatarBounds {
                soul_id: slot.soul_id.clone(),
                aabb: body,
            });
            for part in PARTS {
                if let Some((pmin, pmax)) = slot.avatar.part_world_aabb(part) {
                    interactions.push(InteractionPrimitive::AvatarPart {
                        soul_id: slot.soul_id.clone(),
                        part,
                        aabb: world_aabb_to_px(pmin, pmax, view_proj, viewport),
                    });
                }
            }
            for (kind, bone) in [
                (StageAnchorKind::Head, "head"),
                (StageAnchorKind::Chest, "chest"),
                (StageAnchorKind::LeftHand, "lefthand"),
                (StageAnchorKind::RightHand, "righthand"),
            ] {
                let Some(world) = slot.avatar.overlay_bone_world(bone).or_else(|| {
                    if kind == StageAnchorKind::Head {
                        Some(slot.avatar.head_world())
                    } else {
                        None
                    }
                }) else {
                    continue;
                };
                let (position, offscreen, behind) = project_world_to_px(world, view_proj, viewport);
                anchors.push(StageAnchor {
                    soul_id: slot.soul_id.clone(),
                    kind,
                    position,
                    offscreen,
                    behind_camera: behind,
                });
            }
        }
        if !self.surface.caption.is_empty()
            && let Some(head) = anchors
                .iter()
                .find(|anchor| anchor.kind == StageAnchorKind::Head)
        {
            let rect = PxRect::new(
                head.position.x,
                (head.position.y - 72.0).max(0.0),
                280.0,
                72.0,
            );
            visuals.push(VisualPrimitive::Bubble {
                id: "speech".to_owned(),
                rect,
            });
            interactions.push(InteractionPrimitive::Ui {
                id: "speech".to_owned(),
                rect,
                mode: crate::scene::UiHitMode::DisplayOnly,
            });
        }
        if self.surface.greetings.len() > 1 {
            let rect = PxRect::new(
                24.0,
                140.0,
                280.0,
                40.0 * self.surface.greetings.len() as f32,
            );
            visuals.push(VisualPrimitive::Bubble {
                id: "choices".to_owned(),
                rect,
            });
            interactions.push(InteractionPrimitive::Ui {
                id: "choices".to_owned(),
                rect,
                mode: crate::scene::UiHitMode::Clickable,
            });
        }
        self.scene.set_visuals(visuals);
        self.scene.set_interactions(interactions);
        self.scene.set_anchors(anchors);
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
        let Some(_gpu_ready) = self.gpu.as_ref() else {
            return;
        };

        let chat_rev = self.chat_model_revision();
        if self.chat.as_ref().is_some_and(|chat| {
            chrome_frame_needed(chat.needs_gpu(), self.last_chat_sync, chat_rev)
        }) {
            self.sync_chat_slint();
            if let (Some(gpu), Some(chat)) = (self.gpu.as_ref(), self.chat.as_mut())
                && let Err(err) = chat.paint(gpu)
            {
                tracing::debug!(error = %err, "chat paint failed");
            }
            self.last_chat_sync = Some(chat_rev);
        } else if self.chat.is_none() {
            self.last_chat_sync = None;
        }
        self.drain_chat_actions();

        let detail_rev = self.detail_model_revision();
        if self.detail_win.as_ref().is_some_and(|win| {
            chrome_frame_needed(win.needs_gpu(), self.last_detail_sync, detail_rev)
        }) {
            self.sync_detail_slint();
            if let (Some(gpu), Some(detail_win)) = (self.gpu.as_ref(), self.detail_win.as_mut())
                && let Err(err) = detail_win.paint(gpu)
            {
                tracing::debug!(error = %err, "detail paint failed");
            }
            self.last_detail_sync = Some(detail_rev);
        } else if self.detail_win.is_none() {
            self.last_detail_sync = None;
        }
        let display_action = self.drain_detail_actions();

        let caption_text = surface::caption::speech_text(&self.surface).to_owned();
        let caption_font = self.settings.caption_font_size as i32;
        let caption_pos = self.settings.caption_position.clone();
        let caption_rev = self.caption_model_revision(&caption_text, caption_font, &caption_pos);
        if self.caption.as_ref().is_some_and(|caption| {
            chrome_frame_needed(caption.needs_gpu(), self.last_caption_sync, caption_rev)
        }) {
            if let (Some(gpu), Some(caption)) = (self.gpu.as_ref(), self.caption.as_mut()) {
                if let Some(ui) = caption.layer().and_then(|layer| layer.caption_ui()) {
                    ui.set_caption(caption_text.into());
                    ui.set_font_size(caption_font);
                    apply_theme_caption(ui, &self.local_settings.theme);
                }
                caption.place_caption(&caption_pos);
                if let Err(err) = caption.paint(gpu) {
                    tracing::debug!(error = %err, "caption paint failed");
                }
            }
            self.last_caption_sync = Some(caption_rev);
        } else if self.caption.is_none() {
            self.last_caption_sync = None;
        }

        let spotlight_rev = self.spotlight_model_revision();
        if self.spotlight.as_ref().is_some_and(|spotlight| {
            chrome_frame_needed(
                spotlight.needs_gpu(),
                self.last_spotlight_sync,
                spotlight_rev,
            )
        }) {
            self.sync_spotlight_slint();
            if let (Some(gpu), Some(spotlight)) = (self.gpu.as_ref(), self.spotlight.as_mut())
                && let Err(err) = spotlight.paint(gpu)
            {
                tracing::debug!(error = %err, "spotlight paint failed");
            }
            self.last_spotlight_sync = Some(spotlight_rev);
        } else if self.spotlight.is_none() {
            self.last_spotlight_sync = None;
        }
        if let Some(action) = self.drain_spotlight_actions() {
            self.surface.spotlight_open = false;
            self.spotlight = None;
            match action {
                SpotlightAction::Command(command) => {
                    self.dispatch_shell_command(event_loop, command);
                }
                SpotlightAction::Close => {}
            }
        }
        if let Some(command) = self.detail.motion_command.take() {
            self.apply_motion_command(command);
        }
        if let Some(action) = display_action {
            self.apply_display_action(action);
            if let Some(detail) = self.detail_win.as_ref() {
                detail.request_redraw();
            }
            if let Some(overlay) = self.overlay.as_ref() {
                overlay.window.request_redraw();
            }
        }
    }

    fn sync_chat_slint(&mut self) {
        let Some(chat) = self.chat.as_ref() else {
            return;
        };
        let Some(ui) = chat.layer().and_then(|layer| layer.chat_ui()) else {
            return;
        };
        self.surface.chat_input_focused = ui.get_input_focused();
        if self.surface.push_chat_draft {
            ui.set_draft(self.surface.chat_draft.clone().into());
            self.surface.push_chat_draft = false;
        } else {
            self.surface.chat_draft = ui.get_draft().to_string();
        }
        self.surface.question_draft = ui.get_question_draft().to_string();
        crate::surface::chat::request_single_greeting_commit(&mut self.surface);
        let rows: Vec<crate::ui::ChatRow> =
            normalize_transcript(&self.surface.history, &self.surface.streaming_text)
                .into_iter()
                .map(|row| crate::ui::ChatRow {
                    role: slint::SharedString::from(transcript_label(row.role).as_str()),
                    text: slint::SharedString::from(row.text.as_str()),
                    streaming: row.state == crate::surface::chat::TranscriptState::Streaming,
                    user: row.role == crate::surface::chat::TranscriptKind::User,
                })
                .collect();
        ui.set_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
        let targets: Vec<crate::ui::ChatTargetRow> = self
            .chat_targets()
            .into_iter()
            .map(|target| crate::ui::ChatTargetRow {
                soul_id: slint::SharedString::from(target.soul_id.as_str()),
                label: slint::SharedString::from(target.label.as_str()),
                active: target.active,
            })
            .collect();
        ui.set_targets(slint::ModelRc::new(slint::VecModel::from(targets)));
        ui.set_status(self.surface.status.clone().into());
        let setup = if chat_setup_cta_eligible(&self.surface) {
            crate::detail::chat_setup_gap(&self.surface.chat_setup)
                .map(crate::detail::chat_setup_status)
                .unwrap_or_default()
        } else {
            String::new()
        };
        ui.set_setup_status(setup.into());
        ui.set_turn_active(self.surface.turn_active);
        ui.set_mic_active(self.mic_active);
        ui.set_can_send(composer_send_allowed(&self.surface));
        ui.set_mode(message_mode_index(self.surface.message_mode));
        apply_chat_copy(ui);
        if let Some(pending) = &self.surface.pending_approval {
            ui.set_approval_visible(true);
            ui.set_approval_prompt(
                format!("{}: {} / {}", pending.tool, pending.target, pending.id).into(),
            );
        } else {
            ui.set_approval_visible(false);
            ui.set_approval_prompt(slint::SharedString::new());
        }
        if let Some(question) = &self.surface.pending_question {
            ui.set_question_visible(true);
            ui.set_question_prompt(question.prompt.clone().into());
        } else {
            ui.set_question_visible(false);
        }
        apply_theme_chat(ui, &self.local_settings.theme);
    }

    fn drain_chat_actions(&mut self) {
        let Some(chat) = self.chat.as_ref() else {
            return;
        };
        let actions = chat.take_actions();
        for action in actions {
            match action {
                ChromeAction::Send => self.surface.push_action(SurfaceAction::SendChat),
                ChromeAction::NewSession => self.surface.push_action(SurfaceAction::NewSession),
                ChromeAction::CancelTurn => self.surface.push_action(SurfaceAction::CancelTurn),
                ChromeAction::ToggleMic => self.surface.push_action(SurfaceAction::ToggleMic),
                ChromeAction::OpenDetail => {
                    self.surface
                        .push_action(SurfaceAction::OpenDetail(DetailTab::Home));
                }
                ChromeAction::SelectTarget(soul_id) => {
                    self.surface
                        .push_action(SurfaceAction::SelectCompanion { soul_id });
                }
                ChromeAction::Approval(decision) => {
                    if let Some(pending) = &self.surface.pending_approval {
                        let mapped = match decision.as_str() {
                            "always" => "allow_and_remember",
                            other => other,
                        };
                        self.surface.push_action(SurfaceAction::Approval {
                            id: pending.id.clone(),
                            decision: mapped.to_owned(),
                        });
                    }
                }
                ChromeAction::AnswerQuestion => {
                    self.surface.push_action(SurfaceAction::AnswerQuestion);
                }
                ChromeAction::BargeIn => self.surface.push_action(SurfaceAction::BargeIn),
                ChromeAction::SetMode(index) => {
                    self.surface.message_mode = message_mode_from_index(index);
                }
                _ => {}
            }
        }
    }

    fn sync_detail_slint(&mut self) {
        let Some(win) = self.detail_win.as_ref() else {
            return;
        };
        let Some(ui) = win.layer().and_then(|layer| layer.detail_ui()) else {
            return;
        };
        self.detail.search = ui.get_search().to_string();
        self.session
            .session_id()
            .clone_into(&mut self.detail.session_id);
        self.detail.spotlight_hotkey_ok = self.surface.spotlight_hotkey_ok;
        let soul_id = self.session.soul_id().to_owned();
        let companions = detail::companion_display_rows(
            self.session.occupants(),
            &self.local_settings.displayed_soul_ids,
            &self.temporarily_hidden_souls,
            &soul_id,
        );
        let view = detail::project::project(
            &self.detail,
            &self.local_settings,
            &companions,
            &self.monitors,
        );
        ui.set_heading(view.heading.into());
        ui.set_body(view.body.into());
        ui.set_status(view.status.into());
        let tab_index = DetailTab::ALL
            .iter()
            .position(|tab| *tab == self.detail.tab)
            .unwrap_or(0) as i32;
        if should_push_detail_drafts(ui.get_input_focused(), ui.get_tab(), tab_index) {
            ui.set_draft_a(view.draft_a.into());
            ui.set_draft_b(view.draft_b.into());
        }
        ui.set_draft_a_label(view.draft_a_label.into());
        ui.set_draft_b_label(view.draft_b_label.into());
        ui.set_tab(tab_index);
        ui.set_rows(slint::ModelRc::new(slint::VecModel::from(view.rows)));
        ui.set_drafts_visible(view.drafts_visible);
        let actions: Vec<crate::ui::DetailAction> = view
            .actions
            .into_iter()
            .map(|action| crate::ui::DetailAction {
                id: slint::SharedString::from(action.id),
                label: slint::SharedString::from(action.label),
                primary: action.primary,
            })
            .collect();
        ui.set_actions(slint::ModelRc::new(slint::VecModel::from(actions)));
        let tabs: Vec<slint::SharedString> = DetailTab::ALL
            .iter()
            .map(|tab| slint::SharedString::from(tab.label()))
            .collect();
        ui.set_tabs(slint::ModelRc::new(slint::VecModel::from(tabs)));
        ui.set_search_placeholder(i18n::fl("detail-search").into());
        ui.set_title_text(i18n::fl("detail-title").into());
        apply_theme_detail(ui, &self.local_settings.theme);
    }

    fn drain_detail_actions(&mut self) -> Option<DisplayAction> {
        let win = self.detail_win.as_ref()?;
        let actions = win.take_actions();
        let mut display = None;
        let soul_id = self.session.soul_id().to_owned();
        let companions = detail::companion_display_rows(
            self.session.occupants(),
            &self.local_settings.displayed_soul_ids,
            &self.temporarily_hidden_souls,
            &soul_id,
        );
        for action in actions {
            match action {
                ChromeAction::DetailTab(index) => {
                    self.read_detail_drafts();
                    detail::project::handle_select_tab(&mut self.detail, index);
                }
                ChromeAction::DetailSearchSubmit => {
                    self.detail.search = self
                        .detail_win
                        .as_ref()
                        .and_then(|win| win.layer())
                        .and_then(|layer| layer.detail_ui())
                        .map_or_else(
                            || self.detail.search.clone(),
                            |ui| ui.get_search().to_string(),
                        );
                    crate::detail::sync_search_tab(&mut self.detail);
                }
                ChromeAction::DetailPrimary(name) => {
                    self.read_detail_drafts();
                    if let Some(command) = detail::project::handle_primary(
                        &mut self.detail,
                        &mut self.local_settings,
                        &name,
                    ) {
                        self.dispatch_detail_primary(command);
                    }
                }
                ChromeAction::DetailRow(id) => {
                    if let Some(monitor_id) = id.strip_prefix("monitor:")
                        && let Some(monitor) = self
                            .monitors
                            .iter()
                            .find(|monitor| monitor.id == monitor_id)
                    {
                        OverlayMonitorMode::Selected
                            .setting()
                            .clone_into(&mut self.local_settings.overlay_monitor_mode);
                        self.local_settings.overlay_monitor_id = monitor.id.clone();
                        self.local_settings.overlay_monitor_name =
                            monitor.name.clone().unwrap_or_default();
                        self.local_settings.overlay_monitor_position = monitor.position;
                        self.local_settings.overlay_monitor_size = monitor.size;
                        self.local_settings.overlay_monitor_scale_factor = monitor.scale_factor;
                        detail::request_overlay_monitor_action(&mut self.detail, true);
                    } else if let Some(job_id) = id.strip_prefix("job:") {
                        self.cancel_job(job_id);
                    } else if let Some(model) = id.strip_prefix("model:") {
                        model.clone_into(&mut self.detail.chat_model);
                    } else if let Some(candidate_id) = id.strip_prefix("pending-accept:") {
                        self.resolve_memory_candidate(candidate_id, true);
                    } else if let Some(candidate_id) = id.strip_prefix("pending-reject:") {
                        self.resolve_memory_candidate(candidate_id, false);
                    } else if let Some(action) =
                        detail::project::handle_row(&mut self.detail, &companions, &id)
                    {
                        display = Some(action);
                    }
                }
                _ => {}
            }
        }
        display
    }

    fn read_detail_drafts(&mut self) {
        let (search, draft_a, draft_b, tab) = {
            let Some(win) = self.detail_win.as_ref() else {
                return;
            };
            let Some(ui) = win.layer().and_then(|layer| layer.detail_ui()) else {
                return;
            };
            (
                ui.get_search().to_string(),
                ui.get_draft_a().to_string(),
                ui.get_draft_b().to_string(),
                self.detail.tab,
            )
        };
        self.detail.search = search;
        match tab {
            DetailTab::Conversation => {
                self.detail.chat_plugin = draft_a;
                self.detail.chat_model = draft_b;
            }
            DetailTab::Work => {
                self.detail.new_job_title = draft_a;
                self.detail.new_job_goal = draft_b;
            }
            DetailTab::System => {
                self.local_settings.theme = draft_a;
                self.local_settings.language = draft_b;
            }
            DetailTab::Voice => {
                self.detail.tts_plugin = draft_a;
                self.detail.stt_plugin = draft_b;
            }
            DetailTab::Companion => {
                self.detail.body_ref_draft = draft_a;
            }
            _ => {}
        }
    }

    fn dispatch_detail_primary(&mut self, command: detail::project::DetailPrimary) {
        match command {
            detail::project::DetailPrimary::ApplyAi => {
                detail::apply_ai_patch(
                    &mut self.detail,
                    &self.client,
                    &self.rt_handle,
                    &self.async_results,
                    true,
                );
                detail::apply_observation_patch(
                    &mut self.detail,
                    &self.client,
                    &self.rt_handle,
                    &self.async_results,
                );
            }
            detail::project::DetailPrimary::ApplyVoice => {
                detail::apply_voice_patch(
                    &mut self.detail,
                    &self.client,
                    &self.rt_handle,
                    &self.async_results,
                );
            }
            detail::project::DetailPrimary::ApplySystem => {
                self.detail.save_local_pending = true;
                detail::request_overlay_monitor_action(&mut self.detail, true);
            }
            detail::project::DetailPrimary::ApplyBody => self.apply_body_ref(),
            detail::project::DetailPrimary::CreateJob => self.submit_new_job(),
            detail::project::DetailPrimary::RefreshMemory => self.request_memories(),
            detail::project::DetailPrimary::ReloadMcp => self.reload_mcp_tools(),
            detail::project::DetailPrimary::ReloadJobs => self.request_jobs(),
            detail::project::DetailPrimary::ReloadCharacters => {
                self.request_characters();
                self.request_active_soul();
            }
            detail::project::DetailPrimary::ImportCharacter => self.import_character_dialog(),
            detail::project::DetailPrimary::Reload => {
                detail::ensure_settings(
                    &mut self.detail,
                    &self.client,
                    &self.rt_handle,
                    &self.async_results,
                );
            }
        }
    }

    fn submit_new_job(&mut self) {
        if self.detail.new_job_inflight {
            return;
        }
        let goal = self.detail.new_job_goal.trim().to_owned();
        if goal.is_empty() {
            self.detail.core_status = i18n::fl("jobs-goal-required");
            return;
        }
        let title = self.detail.new_job_title.trim();
        let request = CreateJobRequest {
            soul_id: self.session.soul_id().to_owned(),
            goal,
            title: if title.is_empty() {
                None
            } else {
                Some(title.to_owned())
            },
            success_criteria: Vec::new(),
            allowed_tools: Vec::new(),
        };
        self.detail.new_job_inflight = true;
        self.detail.submitted_job = Some(request.clone());
        let client = Arc::clone(&self.client);
        self.spawn(async move {
            AsyncOutcome::CreateJob(
                client
                    .create_job(&request)
                    .await
                    .map_err(|err| err.to_string()),
            )
        });
    }

    fn resolve_memory_candidate(&mut self, id: &str, accept: bool) {
        let Some(original) = self
            .detail
            .pending_memories
            .iter()
            .find(|candidate| candidate.id == id)
            .cloned()
        else {
            return;
        };
        self.detail.remove_candidate(id);
        let soul_id = self.session.soul_id().to_owned();
        let client = Arc::clone(&self.client);
        let candidate_id = id.to_owned();
        let decision = if accept {
            ene_api::MemoryCandidateDecision::Accept
        } else {
            ene_api::MemoryCandidateDecision::Reject
        };
        self.spawn(async move {
            let result = client
                .resolve_memory_candidate(
                    &candidate_id,
                    &ene_api::ResolveMemoryCandidateRequest {
                        decision,
                        title: None,
                        content: None,
                        kind: None,
                        scope: None,
                    },
                )
                .await
                .map(|_| ())
                .map_err(|err| err.to_string());
            if result.is_err() {
                AsyncOutcome::ResolveMemoryFailedKeepCandidate {
                    soul_id,
                    original,
                    result,
                }
            } else {
                AsyncOutcome::ResolveMemory {
                    soul_id,
                    id: candidate_id,
                    result,
                }
            }
        });
    }

    fn cancel_job(&mut self, id: &str) {
        let id = id.to_owned();
        let client = Arc::clone(&self.client);
        self.spawn(async move {
            AsyncOutcome::CancelJob {
                id: id.clone(),
                result: client
                    .cancel_job(&id)
                    .await
                    .map(|_| ())
                    .map_err(|err| err.to_string()),
            }
        });
    }

    fn reload_mcp_tools(&self) {
        let client = Arc::clone(&self.client);
        self.spawn(async move {
            AsyncOutcome::ReloadMcpTools(
                client
                    .list_tools()
                    .await
                    .map(|page| page.items)
                    .map_err(|err| err.to_string()),
            )
        });
    }

    fn apply_body_ref(&mut self) {
        let soul_id = self.session.soul_id().to_owned();
        let body_ref = self.detail.body_ref_draft.clone();
        let client = Arc::clone(&self.client);
        self.spawn(async move {
            AsyncOutcome::PatchBody(
                client
                    .patch_soul_body(
                        &soul_id,
                        &SoulPatch {
                            body_ref: Some(body_ref),
                        },
                    )
                    .await
                    .map_err(|err| err.to_string()),
            )
        });
    }

    fn import_character_dialog(&mut self) {
        let picked = rfd::FileDialog::new()
            .add_filter("Character", &["enechar", "png", "json", "charx"])
            .pick_file();
        let Some(path) = picked else {
            return;
        };
        self.start_character_import(&path);
    }

    fn start_character_import(&mut self, path: &Path) {
        if self.detail.character_action_pending {
            return;
        }
        self.detail.character_action_pending = true;
        let generation = self.detail.next_activation_generation();
        let client = Arc::clone(&self.client);
        let path = path.to_string_lossy().into_owned();
        self.spawn(async move {
            AsyncOutcome::ImportCharacter {
                generation,
                result: client
                    .import_character(&path)
                    .await
                    .map(|character| crate::tasks::ActivatedCharacter {
                        character,
                        target: None,
                    })
                    .map_err(|err| err.to_string()),
            }
        });
    }

    fn sync_spotlight_slint(&mut self) {
        let Some(spotlight) = self.spotlight.as_ref() else {
            return;
        };
        let Some(ui) = spotlight.layer().and_then(|layer| layer.spotlight_ui()) else {
            return;
        };
        self.surface.spotlight_query = ui.get_query().to_string();
        let entries = surface::spotlight::palette_entries();
        let filtered = surface::spotlight::filter_entries(&self.surface.spotlight_query, &entries);
        let model: Vec<crate::ui::SpotlightEntry> = filtered
            .iter()
            .map(|entry| crate::ui::SpotlightEntry {
                id: slint::SharedString::from(spotlight_entry_id(entry)),
                label: slint::SharedString::from(entry.label.as_str()),
            })
            .collect();
        ui.set_entries(slint::ModelRc::new(slint::VecModel::from(model)));
        let selected = self
            .surface
            .spotlight_selected
            .min(filtered.len().saturating_sub(1));
        self.surface.spotlight_selected = selected;
        ui.set_selected(selected as i32);
        ui.set_placeholder(i18n::fl("spotlight-placeholder").into());
        ui.set_empty_text(i18n::fl("spotlight-no-match").into());
        ui.set_title_text(i18n::fl("spotlight-title").into());
        apply_theme_spotlight(ui, &self.local_settings.theme);
    }

    fn drain_spotlight_actions(&mut self) -> Option<SpotlightAction> {
        let spotlight = self.spotlight.as_ref()?;
        let actions = spotlight.take_actions();
        let query = spotlight
            .layer()
            .and_then(|layer| layer.spotlight_ui())
            .map(|ui| ui.get_query().to_string())
            .unwrap_or_default();
        self.surface.spotlight_query = query;
        let entries = surface::spotlight::palette_entries();
        let filtered = surface::spotlight::filter_entries(&self.surface.spotlight_query, &entries);
        for action in actions {
            match action {
                ChromeAction::SpotlightDismiss => return Some(SpotlightAction::Close),
                ChromeAction::SpotlightRun(id) => {
                    if let Some(entry) = filtered
                        .iter()
                        .find(|entry| spotlight_entry_id(entry) == id)
                    {
                        return Some(entry.action);
                    }
                }
                _ => {}
            }
        }
        None
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
    fn send_chat_paints_the_optimistic_user_row_immediately() {
        let mut app = StageApp::new_for_test();
        app.detail.finish_settings_load();
        app.detail.chat_plugin = "provider.openai_compat".to_owned();
        app.detail.chat_model = "gpt-test".to_owned();
        app.surface.chat_open = true;
        app.surface.chat_draft = "ping-1215".to_owned();

        app.send_chat();

        assert!(
            !app.surface.chat_draft.is_empty(),
            "the editable draft must stay until the send outcome lands"
        );
        assert!(app.surface.turn_active);
        assert_eq!(app.surface.history.messages.len(), 1);
        assert_eq!(app.surface.history.messages[0].role, "user");
        assert_eq!(app.surface.history.messages[0].text, "ping-1215");

        // The success path must leave the already-painted optimistic row in place.
        app.apply_async_outcome(AsyncOutcome::SendMessage {
            session_id: app.session.session_id().to_owned(),
            sent_text: "ping-1215".to_owned(),
            result: Ok(()),
        });
        assert!(app.surface.chat_draft.is_empty());
        assert_eq!(app.surface.history.messages.len(), 1);
        assert_eq!(app.surface.history.messages[0].text, "ping-1215");
    }

    #[test]
    fn repeated_text_gets_a_second_optimistic_row_without_a_success_duplicate() {
        let mut app = StageApp::new_for_test();
        app.detail.finish_settings_load();
        app.detail.chat_plugin = "provider.openai_compat".to_owned();
        app.detail.chat_model = "gpt-test".to_owned();
        app.surface.history.messages.push(ene_api::MessageResponse {
            seq: 4,
            role: "user".to_owned(),
            text: "ping".to_owned(),
        });
        app.surface.chat_draft = "ping".to_owned();

        app.send_chat();

        assert_eq!(app.surface.history.messages.len(), 2);
        assert_eq!(app.surface.history.messages[1].text, "ping");

        app.apply_async_outcome(AsyncOutcome::SendMessage {
            session_id: app.session.session_id().to_owned(),
            sent_text: "ping".to_owned(),
            result: Ok(()),
        });

        assert_eq!(app.surface.history.messages.len(), 2);
        assert_eq!(app.surface.history.messages[1].text, "ping");
    }

    #[test]
    fn failed_send_keeps_the_optimistic_row_and_restores_the_draft() {
        let mut app = StageApp::new_for_test();
        let session_id = app.session.session_id().to_owned();
        app.surface.history.messages.push(ene_api::MessageResponse {
            seq: 4,
            role: "user".to_owned(),
            text: "ping-1215".to_owned(),
        });

        app.apply_async_outcome(AsyncOutcome::SendMessage {
            session_id,
            sent_text: "ping-1215".to_owned(),
            result: Err("transport down".to_owned()),
        });

        assert_eq!(app.surface.status, "transport down");
        assert_eq!(app.surface.chat_draft, "ping-1215");
        assert_eq!(app.surface.history.messages.len(), 1);
        assert_eq!(app.surface.history.messages[0].text, "ping-1215");
    }

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
        self.monitors = monitor::inventory(event_loop);
        self.next_monitor_probe = Instant::now() + Duration::from_millis(500);
        let target = self.overlay_monitor_target(&self.monitors);
        let (position, size) = target.map_or(
            (PhysicalPosition::new(0, 0), PhysicalSize::new(1280, 720)),
            |target| {
                (
                    PhysicalPosition::new(target.rect.position[0], target.rect.position[1]),
                    PhysicalSize::new(target.rect.size[0], target.rect.size[1]),
                )
            },
        );
        let mut attrs = Window::default_attributes()
            .with_title(i18n::fl("app-title"))
            .with_inner_size(size)
            .with_transparent(self.settings.transparent_overlay)
            .with_decorations(!self.settings.transparent_overlay)
            .with_visible(true);
        #[cfg(target_os = "windows")]
        {
            use winit::platform::windows::WindowAttributesExtWindows;
            // DirectComposition owns the visual surface; a redirection bitmap
            // would make the transparent window opaque before the swapchain is composed.
            attrs = attrs.with_no_redirection_bitmap(true);
        }
        attrs = attrs.with_window_level(window_level(self.settings.always_on_top));
        attrs = attrs.with_position(position);
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
        crate::renderer::slint_gpu::install(&gpu);
        match OverlayWindow::create(window, &gpu, self.settings.transparent_overlay) {
            Ok(mut overlay) => {
                let transparency_fallback =
                    self.settings.transparent_overlay && !overlay.transparency_supported;
                overlay.apply_click_through(self.local_settings.overlay_click_through);
                let size = overlay.window.inner_size();
                let mut layer =
                    crate::renderer::slint_gpu::SlintOverlayLayer::new(size.width, size.height);
                layer.ensure_component();
                overlay.set_slint_layer(Some(layer));
                if let Some((_, rx)) = crate::cursor_poll::spawn(50) {
                    self.cursor_poll = Some(rx);
                }
                self.overlay = Some(overlay);
                if let Some(overlay) = self.overlay.as_ref() {
                    self.overlay_platform = OverlayPlatform::attach(&overlay.window);
                }
                if transparency_fallback {
                    self.surface.overlay_notice = i18n::fl("overlay-transparency-unavailable");
                    self.detail.core_status = self.surface.overlay_notice.clone();
                }
            }
            Err(err) => {
                tracing::error!(error = %err, "overlay surface failed");
                event_loop.exit();
                return;
            }
        }
        self.gpu = Some(gpu);
        self.apply_overlay_monitor(event_loop, false);
        self.reload_avatar();
        if self.surface.chat_open {
            self.open_chat(event_loop);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let overlay_id = self.overlay.as_ref().map(OverlayWindow::id);
        if overlay_id == Some(id) {
            if let Some(overlay) = self.overlay.as_ref() {
                let scale = overlay.window.scale_factor();
                if let Some(layer) = overlay.slint_layer() {
                    layer.dispatch_winit(&event, scale);
                }
            }
            if matches!(event, WindowEvent::Focused(true))
                && self.overlay_focus.on_focus_event(FocusOwner::Overlay, true)
            {
                self.sync_overlay_interaction();
            }
            match event {
                WindowEvent::CloseRequested => self.surface.quit = true,
                WindowEvent::Focused(false) => {
                    self.gesture.cancel_all();
                    self.surface.drag = None;
                    self.surface.hover_soul = None;
                    self.interaction.cancel(CancelReason::FocusLost);
                    self.sync_overlay_interaction();
                }
                WindowEvent::CursorLeft { .. } => {
                    if self.surface.drag.is_none() {
                        self.surface.hover_soul = None;
                        self.sync_overlay_interaction();
                    }
                }
                WindowEvent::Resized(size) => {
                    if let (Some(gpu), Some(overlay)) = (self.gpu.as_ref(), self.overlay.as_mut()) {
                        overlay.resize(gpu, size);
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    if self.overlay.as_ref().is_some() {
                        self.on_overlay_cursor_moved(position);
                    }
                }
                WindowEvent::CursorLeft { .. } => {
                    self.surface.hover_soul = None;
                    self.last_cursor = None;
                    self.sync_overlay_interaction();
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => {
                    self.on_overlay_pointer_press_with_protection(PointerKind::Mouse, 0, None);
                }
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button: MouseButton::Left,
                    ..
                } => {
                    self.on_overlay_release();
                }
                WindowEvent::Touch(touch) => match touch.phase {
                    TouchPhase::Started => {
                        self.on_overlay_pointer_press_with_protection(
                            PointerKind::Touch,
                            touch.id,
                            Some(touch.location),
                        );
                    }
                    TouchPhase::Moved => {
                        self.on_overlay_pointer_moved(PointerKind::Touch, touch.id, touch.location);
                    }
                    TouchPhase::Ended => {
                        self.on_overlay_pointer_release(PointerKind::Touch, touch.id);
                    }
                    TouchPhase::Cancelled => {
                        self.cancel_overlay_pointer(PointerKind::Touch, touch.id);
                    }
                },
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
            close_chat = matches!(event, WindowEvent::CloseRequested);
            let repaint = should_repaint_after_event(
                chat.on_window_event(&event),
                self.surface.chat_input_focused,
            );
            if let Some(gpu) = self.gpu.as_ref()
                && let WindowEvent::Resized(size) = &event
            {
                chat.resize(gpu, *size);
            }
            overlay_from_chrome = Some(
                chat.owns_input()
                    || ChromeWindow::composer_owns_keyboard(self.surface.chat_input_focused),
            );
            chrome_focus_state = window_focus_state(&event);
            if repaint && !close_chat {
                chat.request_redraw();
            }
        }
        if let Some(detail) = self.detail_win.as_mut()
            && detail.id() == id
        {
            let repaint = detail.on_window_event(&event);
            if let Some(gpu) = self.gpu.as_ref()
                && let WindowEvent::Resized(size) = &event
            {
                detail.resize(gpu, *size);
            }
            overlay_from_chrome = Some(detail.owns_input());
            chrome_focus_state = window_focus_state(&event);
            close_detail = matches!(event, WindowEvent::CloseRequested);
            if repaint && !close_detail {
                detail.request_redraw();
            }
        }
        if let Some(caption) = self.caption.as_mut()
            && caption.id() == id
        {
            let repaint = caption.on_window_event(&event);
            chrome_focus_state = window_focus_state(&event);
            close_caption = matches!(event, WindowEvent::CloseRequested);
            if repaint && !close_caption {
                caption.request_redraw();
            }
        }
        if let Some(spotlight) = self.spotlight.as_mut()
            && spotlight.id() == id
        {
            let mut skip_slint = false;
            if let WindowEvent::KeyboardInput { event, .. } = &event
                && event.state == ElementState::Pressed
            {
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        close_spotlight = true;
                        skip_slint = true;
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        self.surface.spotlight_selected =
                            self.surface.spotlight_selected.saturating_add(1);
                        skip_slint = true;
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        self.surface.spotlight_selected =
                            self.surface.spotlight_selected.saturating_sub(1);
                        skip_slint = true;
                    }
                    _ => {}
                }
            }
            let repaint = if skip_slint {
                true
            } else {
                should_repaint_after_event(
                    spotlight.on_window_event(&event),
                    spotlight.owns_input(),
                )
            };
            chrome_focus_state = window_focus_state(&event);
            close_spotlight = close_spotlight || matches!(event, WindowEvent::CloseRequested);
            if repaint && !close_spotlight {
                spotlight.request_redraw();
            }
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
        self.drain_async_results();
        self.poll_pending_approvals();
        self.drain_surface_events();
        self.drain_detail_events();
        self.poll_shell(event_loop);
        self.poll_audio();
        self.process_surface_actions(event_loop);
        self.refresh_monitor_inventory(event_loop);
        self.surface.turn_active = self.session.turn_id().is_some();
        if self.surface.history.messages.is_empty() && !self.session.history().messages.is_empty() {
            self.seed_history_from_session();
        }
        if self.overlay_focus.expire_pending_loss(Instant::now()) {
            self.sync_overlay_interaction();
        }
        self.tick_overlay();
        self.paint_chrome(event_loop);
        self.process_overlay_monitor_action(event_loop);
        if self.detail.open_spotlight {
            self.detail.open_spotlight = false;
            self.surface.spotlight_open = true;
            self.ensure_spotlight(event_loop);
        }
        if self.surface.quit {
            event_loop.exit();
            return;
        }
        let overlay_dirty = self
            .overlay
            .as_ref()
            .is_some_and(OverlayWindow::needs_redraw);
        let chrome_dirty = self.chat.as_ref().is_some_and(ChromeWindow::needs_gpu)
            || self
                .detail_win
                .as_ref()
                .is_some_and(ChromeWindow::needs_gpu)
            || self.caption.as_ref().is_some_and(ChromeWindow::needs_gpu)
            || self.spotlight.as_ref().is_some_and(ChromeWindow::needs_gpu);
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + overlay_wake_period(overlay_dirty || chrome_dirty),
        ));
    }
}

fn auth_failure(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("401 unauthorized")
        || lower.contains("403 forbidden")
        || lower.contains("no cookie auth")
}

/// Cyan AABB is drag-only. Hover (including X11 global-cursor poll) must not
/// paint the interaction outline in normal mode.
#[must_use]
fn overlay_outline_soul<'a>(
    drag_soul: Option<&'a str>,
    _hover_soul: Option<&'a str>,
) -> Option<&'a str> {
    drag_soul
}

/// Idle overlay wakes slowly so look-at/blink can still fire without a 60 Hz GPU tick.
#[must_use]
fn overlay_wake_period(needs_redraw: bool) -> Duration {
    if needs_redraw {
        Duration::from_millis(16)
    } else {
        Duration::from_millis(250)
    }
}

#[must_use]
fn chrome_frame_needed(gpu_dirty: bool, last: Option<u64>, revision: u64) -> bool {
    gpu_dirty || last != Some(revision)
}

fn snap_px(value: f32) -> i32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "pixel-quantized overlay dirty compare"
    )]
    {
        value.round() as i32
    }
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

/// Repaint when Slint marked the window dirty, or when a text field is
/// focused so IME/paste is not deferred until the next pointer event.
#[must_use]
fn should_repaint_after_event(repaint_flag: bool, composer_focused: bool) -> bool {
    repaint_flag || composer_focused
}

fn apply_theme_overlay(ui: &crate::ui::StageOverlay, theme: &str) {
    apply_ene_theme(&crate::ui::EneTheme::get(ui), theme);
}

fn apply_theme_chat(ui: &crate::ui::ChatSurface, theme: &str) {
    apply_ene_theme(&crate::ui::EneTheme::get(ui), theme);
}

fn apply_theme_detail(ui: &crate::ui::DetailSurface, theme: &str) {
    apply_ene_theme(&crate::ui::EneTheme::get(ui), theme);
}

fn apply_theme_caption(ui: &crate::ui::CaptionSurface, theme: &str) {
    apply_ene_theme(&crate::ui::EneTheme::get(ui), theme);
}

fn apply_theme_spotlight(ui: &crate::ui::SpotlightSurface, theme: &str) {
    apply_ene_theme(&crate::ui::EneTheme::get(ui), theme);
}

fn apply_ene_theme(global: &crate::ui::EneTheme<'_>, theme: &str) {
    if theme == "light" {
        global.set_dark(false);
        global.set_bg(slint::Color::from_rgb_u8(0xf4, 0xf5, 0xf7));
        global.set_bg_elevated(slint::Color::from_rgb_u8(0xff, 0xff, 0xff));
        global.set_fg(slint::Color::from_rgb_u8(0x1a, 0x1c, 0x20));
        global.set_fg_muted(slint::Color::from_rgb_u8(0x5f, 0x63, 0x68));
        global.set_bubble(slint::Color::from_rgb_u8(0xe8, 0xea, 0xed));
        global.set_bubble_user(slint::Color::from_rgb_u8(0xc5, 0xda, 0xf0));
    } else {
        global.set_dark(true);
        global.set_bg(slint::Color::from_rgb_u8(0x16, 0x18, 0x1d));
        global.set_bg_elevated(slint::Color::from_rgb_u8(0x1e, 0x22, 0x29));
        global.set_fg(slint::Color::from_rgb_u8(0xe8, 0xea, 0xed));
        global.set_fg_muted(slint::Color::from_rgb_u8(0x9a, 0xa0, 0xa6));
        global.set_bubble(slint::Color::from_rgb_u8(0x26, 0x2b, 0x33));
        global.set_bubble_user(slint::Color::from_rgb_u8(0x34, 0x5a, 0x82));
    }
    crate::ui::theme::apply_to_global(
        global,
        &crate::ui::theme::load_user_theme(&user_theme_file()),
    );
}

fn apply_chat_copy(ui: &crate::ui::ChatSurface) {
    ui.set_title_text(i18n::fl("surface-title").into());
    ui.set_detail_label(i18n::fl("chat-open-detail").into());
    ui.set_new_label(i18n::fl("chat-new-session").into());
    ui.set_send_label(i18n::fl("chat-send").into());
    ui.set_cancel_label(i18n::fl("chat-cancel").into());
    ui.set_barge_label(i18n::fl("chat-barge-in").into());
    ui.set_mic_on_label(i18n::fl("mic-on").into());
    ui.set_mic_off_label(i18n::fl("mic-off").into());
    ui.set_allow_label(i18n::fl("approval-allow").into());
    ui.set_always_label(i18n::fl("approval-always").into());
    ui.set_deny_label(i18n::fl("approval-deny").into());
    ui.set_answer_label(i18n::fl("ask-user-answer").into());
    ui.set_placeholder(i18n::fl("chat-placeholder").into());
    ui.set_prompt_label(i18n::fl("chat-mode-prompt").into());
    ui.set_steer_label(i18n::fl("chat-mode-steer").into());
    ui.set_follow_up_label(i18n::fl("chat-mode-follow-up").into());
    ui.set_question_placeholder(i18n::fl("ask-user-answer").into());
}

#[must_use]
fn should_push_detail_drafts(input_focused: bool, ui_tab: i32, state_tab: i32) -> bool {
    ui_tab != state_tab || !input_focused
}

#[must_use]
fn question_answer_text(question_draft: &str, chat_draft: &str, prompt: &str) -> String {
    let question = question_draft.trim();
    if !question.is_empty() {
        return question.to_owned();
    }
    let chat = chat_draft.trim();
    if !chat.is_empty() {
        return chat.to_owned();
    }
    prompt.to_owned()
}

#[must_use]
fn message_mode_index(mode: MessageMode) -> i32 {
    match mode {
        MessageMode::Prompt => 0,
        MessageMode::Steer => 1,
        MessageMode::FollowUp => 2,
    }
}

#[must_use]
fn message_mode_from_index(index: i32) -> MessageMode {
    match index {
        1 => MessageMode::Steer,
        2 => MessageMode::FollowUp,
        _ => MessageMode::Prompt,
    }
}

fn user_theme_file() -> std::path::PathBuf {
    ene_config::data_dir().join("theme.toml")
}

fn spotlight_entry_id(entry: &crate::surface::spotlight::SpotlightEntry) -> String {
    match entry.action {
        SpotlightAction::Close => "close".to_owned(),
        SpotlightAction::Command(ShellCommand::ToggleMic) => "mic".to_owned(),
        SpotlightAction::Command(ShellCommand::OpenChat) => "chat".to_owned(),
        SpotlightAction::Command(ShellCommand::Quit) => "quit".to_owned(),
        SpotlightAction::Command(ShellCommand::OpenDetail(tab)) => format!("detail:{tab:?}"),
        SpotlightAction::Command(_) => entry.label.clone(),
    }
}

/// How long overlay protection survives a chrome Focused(false) while waiting
/// for another chrome window to claim focus during a normal handoff.
const FOCUS_LOSS_GRACE: Duration = Duration::from_millis(200);

fn format_log_text(text: &str) -> &str {
    if text.trim().is_empty() {
        "(empty)"
    } else {
        text
    }
}

#[must_use]
fn disambiguated_occupant_label(occupants: &[ene_api::OccupantView], soul_id: &str) -> String {
    let Some(occupant) = occupants
        .iter()
        .find(|occupant| occupant.soul_id == soul_id)
    else {
        return soul_id.to_owned();
    };
    let base = crate::core::session::occupant_label(occupant);
    let peers = occupants
        .iter()
        .filter(|candidate| crate::core::session::occupant_label(candidate) == base)
        .collect::<Vec<_>>();
    if peers.len() < 2 {
        return base;
    }
    let mut sorted = peers;
    sorted.sort_by_key(|candidate| candidate.soul_id.as_str());
    let pos = sorted
        .iter()
        .position(|candidate| candidate.soul_id == soul_id)
        .unwrap_or(0);
    format!("{base} {}", pos + 1)
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

fn project_world_to_px(
    world: glam::Vec3,
    view_proj: glam::Mat4,
    viewport: (u32, u32),
) -> (glam::Vec2, bool, bool) {
    let clip = view_proj * world.extend(1.0);
    let behind = clip.w <= 0.0;
    let ndc = if clip.w.abs() <= f32::EPSILON {
        glam::Vec2::ZERO
    } else {
        glam::Vec2::new(clip.x / clip.w, clip.y / clip.w)
    };
    let width = viewport.0.max(1) as f32;
    let height = viewport.1.max(1) as f32;
    let mut px = glam::Vec2::new(
        ndc.x.mul_add(0.5, 0.5) * width,
        (0.5 - ndc.y * 0.5) * height,
    );
    let offscreen = px.x < 0.0 || px.y < 0.0 || px.x > width || px.y > height || behind;
    px.x = px.x.clamp(0.0, width);
    px.y = px.y.clamp(0.0, height);
    (px, offscreen, behind)
}

fn world_aabb_to_px(
    min: glam::Vec3,
    max: glam::Vec3,
    view_proj: glam::Mat4,
    viewport: (u32, u32),
) -> PxRect {
    let corners = [
        glam::Vec3::new(min.x, min.y, min.z),
        glam::Vec3::new(min.x, min.y, max.z),
        glam::Vec3::new(min.x, max.y, min.z),
        glam::Vec3::new(min.x, max.y, max.z),
        glam::Vec3::new(max.x, min.y, min.z),
        glam::Vec3::new(max.x, min.y, max.z),
        glam::Vec3::new(max.x, max.y, min.z),
        glam::Vec3::new(max.x, max.y, max.z),
    ];
    let width = viewport.0.max(1) as f32;
    let height = viewport.1.max(1) as f32;
    let mut min_px = glam::Vec2::splat(f32::INFINITY);
    let mut max_px = glam::Vec2::splat(f32::NEG_INFINITY);
    for corner in corners {
        let clip = view_proj * corner.extend(1.0);
        let ndc = if clip.w.abs() <= f32::EPSILON {
            glam::Vec2::ZERO
        } else {
            glam::Vec2::new(clip.x / clip.w, clip.y / clip.w)
        };
        let px = glam::Vec2::new(
            ndc.x.mul_add(0.5, 0.5) * width,
            (0.5 - ndc.y * 0.5) * height,
        );
        min_px = min_px.min(px);
        max_px = max_px.max(px);
    }
    PxRect::new(
        min_px.x,
        min_px.y,
        (max_px.x - min_px.x).max(1.0),
        (max_px.y - min_px.y).max(1.0),
    )
}

fn position_changed(before: [f32; 2], after: [f32; 2]) -> bool {
    before
        .into_iter()
        .zip(after)
        .any(|(before, after)| !before.is_finite() || (before - after).abs() > f32::EPSILON)
}

fn direct_reaction_expression(kind: ReactionKind) -> &'static str {
    match kind {
        ReactionKind::Click => "happy",
        ReactionKind::DoubleClick => "surprised",
        ReactionKind::LongPress => "relaxed",
    }
}

fn direct_reaction_message(kind: ReactionKind) -> &'static str {
    match kind {
        ReactionKind::Click => "The user tapped you. React briefly and warmly if appropriate.",
        ReactionKind::DoubleClick => {
            "The user tapped you twice. React briefly and playfully if appropriate."
        }
        ReactionKind::LongPress => {
            "The user held a touch on you. Acknowledge the gentle contact briefly if appropriate."
        }
    }
}

/// Recognize the daemon's approval-pending rejection of job creation; only
/// this error may stash a request for replay after its approval resolves.
#[must_use]
fn create_denied_by_approval(err: &str) -> bool {
    err.to_ascii_lowercase().contains("job creation denied")
}

/// Map a raw job-creation error to a user-facing reason, translating the
/// approval-pending rejection instead of surfacing the raw `http 403` body.
#[must_use]
fn friendly_create_job_error(err: &str) -> String {
    if create_denied_by_approval(err) {
        i18n::fl("job-creation-denied-by-approval")
    } else {
        err.to_owned()
    }
}

/// Core maps both `AlreadyCompleted` and `Cancelled` to this error class, so a
/// Cancel click that races a finished job arrives as `http 409: already_completed: ...`.
#[must_use]
fn cancel_already_terminal(err: &str) -> bool {
    err.to_ascii_lowercase().contains("already_completed")
}

/// Replace the raw cancel 409 with a short status, keeping unrelated failures intact.
#[must_use]
fn friendly_cancel_job_error(err: &str) -> String {
    if !cancel_already_terminal(err) {
        return err.to_owned();
    }
    let title = err.rsplit_once(':').map_or(err, |(_, title)| title.trim());
    if title.eq_ignore_ascii_case("cancelled") {
        i18n::fl("jobs-cancel-already-cancelled")
    } else {
        i18n::fl("jobs-cancel-already-completed")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AsyncOutcome, ChatWindowAction, FocusOwner, FocusTarget, MAX_COMPLETION_REFRESHES,
        OverlayFocus, StageApp, auth_failure, cancel_already_terminal, chat_window_action,
        chrome_frame_needed, create_denied_by_approval, disambiguated_occupant_label,
        format_log_text, friendly_cancel_job_error, friendly_create_job_error, map_turn_err,
        message_mode_from_index, overlay_outline_soul, overlay_wake_period, overlay_window_level,
        position_changed, project_world_to_px, provider_asset_load_status, question_answer_text,
        should_push_detail_drafts, should_repaint_after_event, spotlight_entry_id, user_theme_file,
        window_focus_state, window_level, world_aabb_to_px,
    };
    use crate::core::events::LiveEvent;
    use crate::core::session::PreparedSessionTarget;
    use crate::detail::PendingJobRetry;
    use crate::i18n;
    use crate::surface::{PendingApproval, PendingQuestion};
    use ene_api::CreateJobRequest;
    use ene_api::{
        HistoryResponse, MemoryCandidateView, MemoryJournalView, MemoryView, MessageMode,
    };
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
    fn window_focus_state_ignores_non_focus_events() {
        assert_eq!(
            window_focus_state(&winit::event::WindowEvent::CloseRequested),
            None
        );
    }

    #[test]
    fn alt_tab_focus_loss_drops_protection_after_grace() {
        // Simulates: user Alt-Tabs away with no recent tray interaction and
        // no other chrome window claiming focus. Protection must drop once
        // the handoff grace expires so the overlay returns to AlwaysOnTop.
        let mut app = StageApp::new_for_test();
        app.chat = None; // ensure chrome_window_exists() can be false
        app.detail_win = None;
        app.caption = None;
        app.spotlight = None;
        app.overlay_focus.transition(FocusTarget::Chat);
        assert!(app.overlay_focus.protects());

        // Focused(false) starts the grace; protection stays during the window.
        let focused_false = window_focus_state(&winit::event::WindowEvent::Focused(false));
        if focused_false == Some(false) {
            app.overlay_focus.on_focus_event(FocusOwner::Chat, false);
        }
        assert!(app.overlay_focus.protects());

        // Once the grace expires with no other claim, protection drops.
        assert!(
            app.overlay_focus
                .expire_pending_loss(Instant::now() + Duration::from_secs(1))
        );
        assert!(!app.overlay_focus.protects());
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
            allowed: false,
            approval_id: String::new(),
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
        app.surface.hover_soul = Some("soul".to_owned());
        let approval = PendingApproval {
            id: "approval".to_owned(),
            tool: "fs.read".to_owned(),
            target: "/tmp/file".to_owned(),
        };

        app.set_pending_approval(approval.clone());

        assert!(app.surface.chat_open);
        assert!(std::mem::take(&mut app.approval_needs_reveal));
        assert!(app.surface.hover_soul.is_none());

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
    fn pending_list_and_badge_share_the_same_api_snapshot() {
        let mut app = StageApp::new_for_test();
        let candidate = |id: &str| MemoryCandidateView {
            id: id.to_owned(),
            soul_id: "soul".to_owned(),
            scope: "private".to_owned(),
            kind: "semantic".to_owned(),
            title: format!("T {id}"),
            content: "C".to_owned(),
            confidence: 0.8,
            sensitive: false,
            expires_at: None,
        };

        app.apply_async_outcome(AsyncOutcome::ListPendingMemories {
            soul_id: "soul".to_owned(),
            result: Ok(vec![candidate("a"), candidate("b")]),
        });
        assert_eq!(app.detail.pending_count(), 2);

        // A later snapshot with fewer rows must shrink both list and badge together.
        app.apply_async_outcome(AsyncOutcome::ListPendingMemories {
            soul_id: "soul".to_owned(),
            result: Ok(vec![candidate("b")]),
        });
        assert_eq!(
            app.detail
                .pending_memories
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["b"]
        );
        assert_eq!(app.detail.pending_count(), 1);
    }

    #[test]
    fn failed_resolve_restores_the_original_candidate_row() {
        let mut app = StageApp::new_for_test();
        let original = MemoryCandidateView {
            id: "candidate-1".to_owned(),
            soul_id: "soul".to_owned(),
            scope: "shared".to_owned(),
            kind: "semantic".to_owned(),
            title: "Original title".to_owned(),
            content: "Original content".to_owned(),
            confidence: 0.9,
            sensitive: true,
            expires_at: None,
        };
        app.detail.pending_memories = vec![original.clone()];

        // Simulates the optimistic removal the UI performs before dispatching resolve.
        app.detail.remove_candidate("candidate-1");
        assert!(app.detail.pending_memories.is_empty());

        app.apply_async_outcome(AsyncOutcome::ResolveMemoryFailedKeepCandidate {
            soul_id: app.session.soul_id().to_owned(),
            original: original.clone(),
            result: Err("server unavailable".to_owned()),
        });

        assert_eq!(app.detail.pending_memories.len(), 1);
        assert_eq!(app.detail.pending_memories[0].title, "Original title");
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
    fn overlay_focus_set_is_idempotent_for_the_same_target() {
        let mut focus = OverlayFocus::default();
        assert!(focus.set(FocusTarget::Chat));
        assert!(!focus.set(FocusTarget::Chat));
        assert!(focus.protects());
    }

    #[test]
    fn mark_pending_loss_is_a_noop_without_a_target() {
        let mut focus = OverlayFocus::default();
        assert!(!focus.mark_pending_loss());
        assert!(!focus.expire_pending_loss(Instant::now() + Duration::from_secs(1)));
    }

    #[test]
    fn clear_target_only_drops_the_matching_chrome() {
        let mut focus = OverlayFocus::default();
        focus.transition(FocusTarget::Chat);
        focus.clear_target(FocusTarget::Detail);
        assert!(focus.protects());
        focus.clear_target(FocusTarget::Chat);
        assert!(!focus.protects());
    }

    #[test]
    fn cancel_pending_loss_keeps_the_current_target() {
        let mut focus = OverlayFocus::default();
        focus.transition(FocusTarget::Caption);
        assert!(!focus.on_focus_event(FocusOwner::Caption, false));
        focus.cancel_pending_loss();
        assert!(!focus.expire_pending_loss(Instant::now() + Duration::from_secs(1)));
        assert!(focus.protects());
    }

    #[test]
    fn spotlight_and_caption_focus_protect_the_overlay() {
        let mut focus = OverlayFocus::default();
        assert!(focus.on_focus_event(FocusOwner::Spotlight, true));
        assert!(focus.protects());
        assert!(focus.on_focus_event(FocusOwner::Caption, true));
        assert!(focus.protects());
        assert!(!focus.on_focus_event(FocusOwner::Overlay, false));
        assert!(focus.protects());
    }

    #[test]
    fn hover_does_not_paint_the_cyan_outline() {
        assert_eq!(overlay_outline_soul(None, Some("soul")), None);
        assert_eq!(
            overlay_outline_soul(Some("drag"), Some("hover")),
            Some("drag")
        );
        assert_eq!(overlay_outline_soul(None, None), None);
    }

    #[test]
    fn chrome_window_exists_is_false_without_chrome() {
        let app = StageApp::new_for_test();
        assert!(!app.chrome_window_exists());
        assert_eq!(app.visible_display_count(), 0);
        assert!(!app.has_avatar_occupant("soul"));
        assert!(app.chat_targets().is_empty());
        assert_eq!(app.companion_label_for_soul("soul"), "soul");
        assert_eq!(app.active_companion_label(), "soul");
        assert!(app.overlay_motion_controls().is_empty());
    }

    #[test]
    fn auth_failure_matches_http_and_cookie_messages() {
        assert!(auth_failure("401 Unauthorized"));
        assert!(auth_failure("403 Forbidden"));
        assert!(auth_failure("NO COOKIE AUTH"));
        assert!(!auth_failure("timeout"));
        assert!(!auth_failure(""));
    }

    #[test]
    fn map_turn_err_translates_known_classes() {
        assert_eq!(
            map_turn_err("no_active_operation"),
            i18n::fl("chat-no-active-turn")
        );
        assert_eq!(
            map_turn_err("401 Unauthorized"),
            i18n::fl("chat-auth-failed")
        );
        assert_eq!(map_turn_err("boom"), "boom");
    }

    #[test]
    fn position_changed_uses_epsilon_and_rejects_nan() {
        assert!(!position_changed([1.0, 2.0], [1.0, 2.0]));
        assert!(position_changed([1.0, 2.0], [1.0, 2.1]));
        assert!(position_changed([f32::NAN, 0.0], [0.0, 0.0]));
        assert!(!position_changed(
            [1.0, 2.0],
            [1.0 + f32::EPSILON / 2.0, 2.0]
        ));
    }

    #[test]
    fn project_world_to_px_handles_behind_and_zero_viewport() {
        let identity = glam::Mat4::IDENTITY;
        let (px, offscreen, behind) = project_world_to_px(glam::Vec3::ZERO, identity, (100, 50));
        assert!(!behind);
        assert!(!offscreen);
        assert!((px.x - 50.0).abs() < 1e-4);
        assert!((px.y - 25.0).abs() < 1e-4);

        let behind_m = glam::Mat4::from_cols(
            glam::Vec4::X,
            glam::Vec4::Y,
            glam::Vec4::Z,
            glam::Vec4::new(0.0, 0.0, 0.0, -1.0),
        );
        let (behind_px, offscreen_behind, is_behind) =
            project_world_to_px(glam::Vec3::ZERO, behind_m, (0, 0));
        assert!(is_behind);
        assert!(offscreen_behind);
        assert!(behind_px.x >= 0.0 && behind_px.y >= 0.0);
    }

    #[test]
    fn world_aabb_to_px_has_minimum_extent() {
        let rect = world_aabb_to_px(
            glam::Vec3::ZERO,
            glam::Vec3::ZERO,
            glam::Mat4::IDENTITY,
            (100, 100),
        );
        assert!(rect.w >= 1.0);
        assert!(rect.h >= 1.0);
    }

    #[test]
    fn create_denied_by_approval_is_case_insensitive() {
        assert!(create_denied_by_approval("Job Creation Denied"));
        assert!(!create_denied_by_approval("forbidden"));
    }

    #[test]
    fn user_theme_file_is_under_the_data_dir() {
        let path = user_theme_file();
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("theme.toml")
        );
    }

    #[test]
    fn spotlight_entry_id_is_stable_for_known_actions() {
        use crate::shell::ShellCommand;
        use crate::surface::spotlight::SpotlightEntry;
        assert_eq!(
            spotlight_entry_id(&SpotlightEntry {
                label: "x".into(),
                action: crate::surface::SpotlightAction::Close,
                keywords: &[],
                shortcut: None,
            }),
            "close"
        );
        assert_eq!(
            spotlight_entry_id(&SpotlightEntry {
                label: "x".into(),
                action: crate::surface::SpotlightAction::Command(ShellCommand::ToggleMic),
                keywords: &[],
                shortcut: None,
            }),
            "mic"
        );
        assert_eq!(
            spotlight_entry_id(&SpotlightEntry {
                label: "fallback".into(),
                action: crate::surface::SpotlightAction::Command(ShellCommand::Quit),
                keywords: &[],
                shortcut: None,
            }),
            "quit"
        );
    }

    #[test]
    fn disambiguated_occupant_label_suffixes_duplicates() {
        let occupants = vec![
            ene_api::OccupantView {
                soul_id: "a".into(),
                display_name: String::new(),
                body_id: None,
                package_id: Some("char.Twin".into()),
                avatar_path: None,
            },
            ene_api::OccupantView {
                soul_id: "b".into(),
                display_name: String::new(),
                body_id: None,
                package_id: Some("char.Twin".into()),
                avatar_path: None,
            },
        ];
        assert_eq!(disambiguated_occupant_label(&occupants, "a"), "Twin 1");
        assert_eq!(disambiguated_occupant_label(&occupants, "b"), "Twin 2");
        assert_eq!(
            disambiguated_occupant_label(&occupants, "missing"),
            "missing"
        );
    }

    #[test]
    fn idle_overlay_wakes_slower_than_a_dirty_frame() {
        assert_eq!(overlay_wake_period(true), Duration::from_millis(16));
        assert_eq!(overlay_wake_period(false), Duration::from_millis(250));
    }

    #[test]
    fn chrome_frame_needed_skips_unchanged_idle_windows() {
        assert!(chrome_frame_needed(true, Some(1), 1));
        assert!(chrome_frame_needed(false, None, 1));
        assert!(chrome_frame_needed(false, Some(1), 2));
        assert!(!chrome_frame_needed(false, Some(1), 1));
    }

    #[test]
    fn overlay_focus_loses_protection_when_overlay_gains_focus() {
        let mut focus = OverlayFocus::default();
        focus.transition(FocusTarget::Chat);

        assert!(focus.on_focus_event(FocusOwner::Overlay, true));
        assert!(!focus.protects());
    }

    #[test]
    fn chrome_focus_loss_keeps_protection_until_grace_expires() {
        let mut focus = OverlayFocus::default();
        focus.transition(FocusTarget::Chat);

        // Detail gaining focus replaces the target without dropping protection.
        assert!(focus.on_focus_event(FocusOwner::Detail, true));
        assert!(focus.protects());

        // Detail losing focus starts the grace period; protection stays up and
        // no interaction sync is reported because the overlay must not flip.
        assert!(!focus.on_focus_event(FocusOwner::Detail, false));
        assert!(focus.protects());

        // A stale Chat loss must not disturb the pending handoff.
        assert!(!focus.on_focus_event(FocusOwner::Chat, false));
        assert!(focus.protects());

        // After the grace expires, protection drops.
        assert!(focus.expire_pending_loss(Instant::now() + Duration::from_secs(1)));
        assert!(!focus.protects());
    }

    #[test]
    fn ordered_chrome_handoff_never_drops_protection() {
        let mut focus = OverlayFocus::default();
        focus.transition(FocusTarget::Chat);
        assert!(focus.protects());

        // Ordered Chat false -> Detail true: the normal OS event order.
        // Protection must remain continuously true across both events.
        assert!(!focus.on_focus_event(FocusOwner::Chat, false));
        assert!(
            focus.protects(),
            "protection must survive transient focus loss"
        );
        assert!(focus.on_focus_event(FocusOwner::Detail, true));
        assert!(focus.protects());

        // The new claim cancels the pending-loss grace; expiring later is a no-op.
        assert!(!focus.expire_pending_loss(Instant::now() + Duration::from_secs(1)));
        assert!(focus.protects());
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
        assert_eq!(app.surface.history.messages[0].text, "kept");
    }

    #[test]
    fn empty_surface_is_seeded_from_session_snapshot_once_available() {
        let mut app = StageApp::new_for_test();
        assert!(app.session.history().messages.is_empty());

        // A turn completed before the surface ever rendered, so the boot-time
        // snapshot was fetched empty; the cache fills only after a refresh.
        app.session.replace_history(HistoryResponse {
            messages: vec![ene_api::MessageResponse {
                seq: 1,
                role: "assistant".to_owned(),
                text: "completed before paint".to_owned(),
            }],
            depth: "surface".to_owned(),
        });

        app.seed_history_from_session();
        assert_eq!(app.surface.history.messages.len(), 1);
        assert_eq!(
            app.surface.history.messages[0].text,
            "completed before paint"
        );

        // Seeding is one-shot: later refreshes own the surface rows.
        app.surface.history = HistoryResponse {
            messages: Vec::new(),
            depth: "surface".to_owned(),
        };
        app.session.replace_history(HistoryResponse {
            messages: Vec::new(),
            depth: "surface".to_owned(),
        });
        app.seed_history_from_session();
        assert!(app.surface.history.messages.is_empty());
    }

    #[test]
    fn completion_refresh_replaces_optimistic_user_row_with_assistant_row() {
        let mut app = StageApp::new_for_test();
        let session_id = app.session.session_id().to_owned();

        // The turn ends before the SendMessage outcome is drained, so the
        // event handler must arm reconciliation without seeing a turn id.
        app.session.set_turn_id_for_test("turn-1");
        app.apply_live_event(LiveEvent::SessionEvent {
            kind: "turn/end".to_owned(),
            text: String::new(),
        });
        assert_eq!(
            app.pending_completion_refreshes,
            MAX_COMPLETION_REFRESHES - 1
        );
        assert!(app.completion_reconcile_inflight);

        // The late send success still owns the optimistic row and re-arms the
        // same loop instead of starting a competing one.
        app.apply_async_outcome(AsyncOutcome::SendMessage {
            session_id,
            sent_text: "again".to_owned(),
            result: Ok(()),
        });
        assert_eq!(
            app.pending_completion_refreshes,
            MAX_COMPLETION_REFRESHES - 1
        );
        assert_eq!(app.surface.history.messages.len(), 1);
        assert_eq!(app.surface.history.messages[0].text, "again");

        // Attempt #1 (from turn/end) returns an empty first-turn history. The
        // optimistic surface stays because there is no assistant row yet, and
        // the retry loop must reschedule.
        let stale = HistoryResponse {
            messages: Vec::new(),
            depth: "surface".to_owned(),
        };
        app.apply_async_outcome(AsyncOutcome::ReconcileHistory {
            session_id: app.session.session_id().to_owned(),
            result: Ok(stale),
        });
        assert_eq!(app.surface.history.messages.len(), 1);
        assert_eq!(app.surface.history.messages[0].text, "again");
        assert_eq!(
            app.pending_completion_refreshes,
            MAX_COMPLETION_REFRESHES - 2
        );
        assert!(app.completion_reconcile_inflight);

        // Attempt #2 finally carries the completed assistant row, owns the
        // surface, and stops the loop before the budget drains.
        let with_assistant = HistoryResponse {
            messages: vec![
                ene_api::MessageResponse {
                    seq: 2,
                    role: "user".to_owned(),
                    text: "again".to_owned(),
                },
                ene_api::MessageResponse {
                    seq: 3,
                    role: "assistant".to_owned(),
                    text: "done".to_owned(),
                },
            ],
            depth: "surface".to_owned(),
        };
        app.apply_async_outcome(AsyncOutcome::ReconcileHistory {
            session_id: app.session.session_id().to_owned(),
            result: Ok(with_assistant.clone()),
        });
        assert_eq!(
            app.surface.history.messages.len(),
            with_assistant.messages.len()
        );
        assert_eq!(
            app.surface.history.messages.last().map(|m| m.text.as_str()),
            Some("done")
        );
        assert_eq!(app.pending_completion_refreshes, 0);
    }

    #[test]
    fn failed_reconcile_keeps_optimistic_rows_for_the_next_attempt() {
        let mut app = StageApp::new_for_test();
        app.apply_async_outcome(AsyncOutcome::SendMessage {
            session_id: app.session.session_id().to_owned(),
            sent_text: "kept".to_owned(),
            result: Ok(()),
        });
        assert_eq!(app.surface.history.messages.len(), 1);

        // A rejected history fetch must leave the surface untouched so the
        // optimistic row survives until a later attempt carries the turn.
        app.apply_async_outcome(AsyncOutcome::ReconcileHistory {
            session_id: app.session.session_id().to_owned(),
            result: Err("connection refused".to_owned()),
        });
        assert_eq!(app.surface.history.messages.len(), 1);
        assert_eq!(app.surface.history.messages[0].text, "kept");
        // The failed attempt consumes one slot of the bounded budget and
        // re-arms the loop right away instead of ending reconciliation.
        assert_eq!(
            app.pending_completion_refreshes,
            MAX_COMPLETION_REFRESHES - 2
        );
        assert!(app.completion_reconcile_inflight);
    }

    #[test]
    fn plain_refresh_between_turns_applies_completion_guards() {
        let mut app = StageApp::new_for_test();
        let session_id = app.session.session_id().to_owned();
        app.pending_completion_refreshes = MAX_COMPLETION_REFRESHES;

        // A user-issued refresh that races the completion loop still shows an
        // assistant-bearing projection; unrelated refreshes take the guard.
        app.apply_async_outcome(AsyncOutcome::RefreshHistory {
            session_id,
            result: Ok(HistoryResponse {
                messages: vec![ene_api::MessageResponse {
                    seq: 7,
                    role: "assistant".to_owned(),
                    text: "done".to_owned(),
                }],
                depth: "surface".to_owned(),
            }),
        });
        assert_eq!(app.surface.history.messages[0].text, "done");
        assert_eq!(app.pending_completion_refreshes, 0);
    }

    #[test]
    fn reconciliation_ignores_stale_prior_turns_and_waits_for_new_rows() {
        let mut app = StageApp::new_for_test();
        let session_id = app.session.session_id().to_owned();

        // The conversation already holds a completed first turn. Surface seqs
        // mirror the sparse event log: the hidden prior turn-end consumed seq
        // 11, so the next surface row will not start at 3.
        app.session.replace_history(HistoryResponse {
            messages: vec![
                ene_api::MessageResponse {
                    seq: 10,
                    role: "user".to_owned(),
                    text: "first question".to_owned(),
                },
                ene_api::MessageResponse {
                    seq: 12,
                    role: "assistant".to_owned(),
                    text: "first answer".to_owned(),
                },
            ],
            depth: "surface".to_owned(),
        });
        app.surface.history = app.session.history();

        // Second turn completes before the send outcome drains.
        app.session.set_turn_id_for_test("turn-2");
        app.apply_live_event(LiveEvent::SessionEvent {
            kind: "turn/end".to_owned(),
            text: String::new(),
        });
        app.apply_async_outcome(AsyncOutcome::SendMessage {
            session_id,
            sent_text: "second question".to_owned(),
            result: Ok(()),
        });
        assert_eq!(app.surface.history.messages.len(), 3);
        assert_eq!(app.surface.history.messages[2].text, "second question");

        // Attempt #1 returns the stale pre-turn rows (no new user row yet):
        // the optimistic row must stay and the loop must retry.
        let stale_pair = HistoryResponse {
            messages: vec![
                ene_api::MessageResponse {
                    seq: 10,
                    role: "user".to_owned(),
                    text: "first question".to_owned(),
                },
                ene_api::MessageResponse {
                    seq: 12,
                    role: "assistant".to_owned(),
                    text: "first answer".to_owned(),
                },
            ],
            depth: "surface".to_owned(),
        };
        app.apply_async_outcome(AsyncOutcome::ReconcileHistory {
            session_id: app.session.session_id().to_owned(),
            result: Ok(stale_pair.clone()),
        });
        assert_eq!(app.surface.history.messages.len(), 3);
        assert_eq!(app.surface.history.messages[2].text, "second question");
        assert!(app.completion_reconcile_inflight);

        // The authoritative user row (seq 13) sits above the synthetic
        // optimistic seq but is not a terminal row, so the loop must keep
        // waiting even though "progress" happened.
        let user_only = HistoryResponse {
            messages: vec![
                ene_api::MessageResponse {
                    seq: 10,
                    role: "user".to_owned(),
                    text: "first question".to_owned(),
                },
                ene_api::MessageResponse {
                    seq: 12,
                    role: "assistant".to_owned(),
                    text: "first answer".to_owned(),
                },
                ene_api::MessageResponse {
                    seq: 13,
                    role: "user".to_owned(),
                    text: "second question".to_owned(),
                },
            ],
            depth: "surface".to_owned(),
        };
        app.apply_async_outcome(AsyncOutcome::ReconcileHistory {
            session_id: app.session.session_id().to_owned(),
            result: Ok(user_only),
        });
        assert_eq!(app.surface.history.messages.len(), 3);
        assert_eq!(app.surface.history.messages[2].text, "second question");
        assert!(app.completion_reconcile_inflight);

        // Attempt #3 carries the newly completed turn and owns the surface.
        let with_second_turn = HistoryResponse {
            messages: vec![
                ene_api::MessageResponse {
                    seq: 10,
                    role: "user".to_owned(),
                    text: "first question".to_owned(),
                },
                ene_api::MessageResponse {
                    seq: 12,
                    role: "assistant".to_owned(),
                    text: "first answer".to_owned(),
                },
                ene_api::MessageResponse {
                    seq: 13,
                    role: "user".to_owned(),
                    text: "second question".to_owned(),
                },
                ene_api::MessageResponse {
                    seq: 15,
                    role: "assistant".to_owned(),
                    text: "second answer".to_owned(),
                },
            ],
            depth: "surface".to_owned(),
        };
        app.apply_async_outcome(AsyncOutcome::ReconcileHistory {
            session_id: app.session.session_id().to_owned(),
            result: Ok(with_second_turn.clone()),
        });
        assert_eq!(app.surface.history.messages.len(), 4);
        assert_eq!(app.surface.history.messages[3].text, "second answer");
        assert_eq!(app.pending_completion_refreshes, 0);
        assert!(!app.completion_reconcile_inflight);
    }

    #[test]
    fn new_session_resets_stale_completion_terminal() {
        let mut app = StageApp::new_for_test();

        // A prior session ended with a high assistant seq; its terminal
        // marker must not leak into a fresh session.
        app.completion_terminal_seq = Some(100);

        app.start_new_session();
        let session_view = |id: &str| ene_api::SessionView {
            id: id.to_owned(),
            soul_id: "soul".to_owned(),
            kind: "conversation".to_owned(),
            title: None,
            created_at: String::new(),
            archived: false,
            next_seq: 0,
            ended_at: None,
            end_reason: None,
            delegation_id: None,
        };
        let split = ene_api::SplitSessionResponse {
            previous: session_view("old-session"),
            session: session_view("newer-session"),
        };
        app.apply_async_outcome(AsyncOutcome::NewSession(Ok(split)));
        assert!(!app.completion_reconcile_inflight);
        assert_eq!(app.pending_completion_refreshes, 0);
        assert_eq!(app.completion_terminal_seq, None);

        // First turn in the fresh session: a user-only fetch at seq 5 must
        // not count as completion.
        app.begin_completion_reconciliation();
        let user_only = HistoryResponse {
            messages: vec![ene_api::MessageResponse {
                seq: 5,
                role: "user".to_owned(),
                text: "hello".to_owned(),
            }],
            depth: "surface".to_owned(),
        };
        app.apply_async_outcome(AsyncOutcome::ReconcileHistory {
            session_id: app.session.session_id().to_owned(),
            result: Ok(user_only),
        });
        assert!(app.completion_reconcile_inflight);
        assert_eq!(
            app.pending_completion_refreshes,
            MAX_COMPLETION_REFRESHES - 2
        );

        // The assistant reply at seq 6 completes reconciliation.
        let with_reply = HistoryResponse {
            messages: vec![
                ene_api::MessageResponse {
                    seq: 5,
                    role: "user".to_owned(),
                    text: "hello".to_owned(),
                },
                ene_api::MessageResponse {
                    seq: 6,
                    role: "assistant".to_owned(),
                    text: "hi".to_owned(),
                },
            ],
            depth: "surface".to_owned(),
        };
        app.apply_async_outcome(AsyncOutcome::ReconcileHistory {
            session_id: app.session.session_id().to_owned(),
            result: Ok(with_reply),
        });
        assert_eq!(
            app.surface.history.messages.last().map(|m| m.text.as_str()),
            Some("hi")
        );
        assert_eq!(app.pending_completion_refreshes, 0);
        assert!(!app.completion_reconcile_inflight);
    }

    #[test]
    fn reconcile_ignores_other_sessions_and_drains_budget_when_assistant_missing() {
        let mut app = StageApp::new_for_test();
        let session_id = app.session.session_id().to_owned();

        // The conversation already holds an assistant row before this turn,
        // so only a strictly newer assistant row can complete it.
        app.session.replace_history(HistoryResponse {
            messages: vec![ene_api::MessageResponse {
                seq: 1,
                role: "assistant".to_owned(),
                text: "old row".to_owned(),
            }],
            depth: "surface".to_owned(),
        });
        app.begin_completion_reconciliation();
        assert_eq!(
            app.pending_completion_refreshes,
            MAX_COMPLETION_REFRESHES - 1
        );

        // A result for another session must not touch this session's state.
        app.apply_async_outcome(AsyncOutcome::ReconcileHistory {
            session_id: "other-session".to_owned(),
            result: Ok(HistoryResponse {
                messages: Vec::new(),
                depth: "surface".to_owned(),
            }),
        });

        // The in-flight attempt is still pending, and an assistant row at the
        // pre-turn terminal seq does not count as new completion.
        app.apply_async_outcome(AsyncOutcome::ReconcileHistory {
            session_id: session_id.clone(),
            result: Ok(HistoryResponse {
                messages: vec![ene_api::MessageResponse {
                    seq: 1,
                    role: "assistant".to_owned(),
                    text: "old row".to_owned(),
                }],
                depth: "surface".to_owned(),
            }),
        });
        assert_eq!(app.surface.history.messages.len(), 1);
        // The remaining budget drains without an assistant row and stops
        // bounded.
        for _ in 0..MAX_COMPLETION_REFRESHES - 2 {
            app.apply_async_outcome(AsyncOutcome::ReconcileHistory {
                session_id: session_id.clone(),
                result: Ok(HistoryResponse {
                    messages: Vec::new(),
                    depth: "surface".to_owned(),
                }),
            });
        }
        assert!(!app.completion_reconcile_inflight);
        assert_eq!(app.pending_completion_refreshes, 0);
        assert_eq!(app.surface.history.messages.len(), 1);
        assert_eq!(app.surface.history.messages[0].text, "old row");

        // The final attempt's outcome clears the loop for good.
        app.apply_async_outcome(AsyncOutcome::ReconcileHistory {
            session_id,
            result: Ok(HistoryResponse {
                messages: Vec::new(),
                depth: "surface".to_owned(),
            }),
        });
        assert!(!app.completion_reconcile_inflight);
    }

    #[test]
    fn activate_reports_activated_status_not_imported() {
        let mut app = StageApp::new_for_test();
        app.detail.character_action_pending = true;
        let generation = app.detail.next_activation_generation();
        let character = ene_api::CharacterView {
            id: "char.alicia-b".to_owned(),
            version: "1.0.0".to_owned(),
            kind: "package".to_owned(),
            path: "/packages/char.alicia-b@1.0.0".to_owned(),
            soul_id: Some("alicia-b".to_owned()),
        };
        app.apply_async_outcome(AsyncOutcome::ActivateCharacter {
            generation,
            result: Ok(crate::tasks::ActivatedCharacter {
                character,
                target: None,
            }),
        });
        assert!(
            app.detail.core_status.contains("char.alicia-b"),
            "status missing character id: {}",
            app.detail.core_status
        );
        assert!(
            app.detail.core_status.contains("Activated character")
                || app.detail.core_status.contains("有効化しました"),
            "activate should not report an import message: {}",
            app.detail.core_status
        );
        assert!(
            !app.detail.core_status.contains("Imported character")
                && !app.detail.core_status.contains("インポートしました"),
            "activate must not reuse the import message: {}",
            app.detail.core_status
        );
        assert!(!app.detail.character_action_pending);
    }

    #[test]
    fn display_save_preserves_action_status() {
        let mut app = StageApp::new_for_test();
        app.local_settings_save_generation = 1;
        let status = i18n::fl("character-display-updated");
        app.apply_async_outcome(AsyncOutcome::SaveLocalSettings {
            generation: 1,
            result: Ok(()),
            success_status: Some(status.clone()),
        });
        assert_eq!(app.detail.core_status, status);
    }

    #[test]
    fn stale_settings_save_cannot_overwrite_newer_status() {
        let mut app = StageApp::new_for_test();
        let status = i18n::fl("character-removed-from-display");
        app.local_settings_save_generation = 2;
        app.detail.core_status.clone_from(&status);
        app.apply_async_outcome(AsyncOutcome::SaveLocalSettings {
            generation: 1,
            result: Ok(()),
            success_status: None,
        });
        assert_eq!(app.detail.core_status, status);
    }

    #[test]
    fn import_reports_imported_status() {
        let mut app = StageApp::new_for_test();
        let generation = app.detail.next_activation_generation();
        let character = ene_api::CharacterView {
            id: "char.alicia".to_owned(),
            version: "1.0.0".to_owned(),
            kind: "package".to_owned(),
            path: "/packages/char.alicia@1.0.0".to_owned(),
            soul_id: Some("alicia".to_owned()),
        };
        app.apply_async_outcome(AsyncOutcome::ImportCharacter {
            generation,
            result: Ok(crate::tasks::ActivatedCharacter {
                character,
                target: None,
            }),
        });
        assert!(
            app.detail.core_status.contains("Imported character")
                || app.detail.core_status.contains("インポートしました"),
            "import should report the import message: {}",
            app.detail.core_status
        );
    }

    #[test]
    fn empty_job_goal_does_not_submit() {
        let mut app = StageApp::new_for_test();
        app.detail.new_job_title = "Plant care".to_owned();
        app.submit_new_job();
        assert!(!app.detail.new_job_inflight);
        assert!(app.detail.submitted_job.is_none());
        assert_eq!(app.detail.core_status, i18n::fl("jobs-goal-required"));
    }

    #[test]
    fn work_submit_records_create_job_not_ai_patch() {
        let mut app = StageApp::new_for_test();
        app.detail.new_job_title = "Plant care".to_owned();
        app.detail.new_job_goal = "water the plants".to_owned();
        app.dispatch_detail_primary(crate::detail::project::DetailPrimary::CreateJob);
        assert!(app.detail.new_job_inflight);
        let submitted = app
            .detail
            .submitted_job
            .as_ref()
            .expect("create-job must stash the request");
        assert_eq!(submitted.goal, "water the plants");
        assert_eq!(submitted.title.as_deref(), Some("Plant care"));
        assert_eq!(submitted.soul_id, "soul");
    }

    #[test]
    fn create_job_denied_by_approval_gets_user_facing_message() {
        let raw = "http 403: forbidden: job creation denied";
        assert_eq!(
            friendly_create_job_error(raw),
            i18n::fl("job-creation-denied-by-approval")
        );
        // Unrelated errors pass through unchanged.
        assert_eq!(
            friendly_create_job_error("http 500: internal error"),
            "http 500: internal error"
        );
    }

    #[test]
    fn cancel_already_terminal_matches_core_conflict_class() {
        assert!(cancel_already_terminal(
            "http 409: already_completed: already completed"
        ));
        assert!(cancel_already_terminal(
            "http 409: already_completed: cancelled"
        ));
        assert!(!cancel_already_terminal(
            "http 409: question_closed: no open question"
        ));
        assert!(!cancel_already_terminal("http 500: fault: boom"));
    }

    #[test]
    fn cancel_of_completed_job_uses_friendly_status_instead_of_raw_http() {
        let mut app = StageApp::new_for_test();
        app.detail.core_status = "stale".to_owned();
        app.apply_async_outcome(AsyncOutcome::CancelJob {
            id: "job-1".to_owned(),
            result: Err("http 409: already_completed: already completed".to_owned()),
        });
        assert_eq!(
            app.detail.core_status,
            i18n::fl("jobs-cancel-already-completed")
        );
        assert!(
            !app.detail.core_status.contains("http 409"),
            "raw conflict must not reach the status row: {}",
            app.detail.core_status
        );
    }

    #[test]
    fn cancel_of_cancelled_job_uses_friendly_status() {
        let mut app = StageApp::new_for_test();
        app.apply_async_outcome(AsyncOutcome::CancelJob {
            id: "job-1".to_owned(),
            result: Err("http 409: already_completed: cancelled".to_owned()),
        });
        assert_eq!(
            app.detail.core_status,
            i18n::fl("jobs-cancel-already-cancelled")
        );
    }

    #[test]
    fn cancel_job_other_errors_stay_raw() {
        let mut app = StageApp::new_for_test();
        let raw = "http 500: fault: boom";
        app.apply_async_outcome(AsyncOutcome::CancelJob {
            id: "job-1".to_owned(),
            result: Err(raw.to_owned()),
        });
        assert_eq!(app.detail.core_status, raw);
        assert_eq!(friendly_cancel_job_error(raw), raw);
    }

    fn submitted_request(goal: &str) -> CreateJobRequest {
        CreateJobRequest {
            soul_id: "soul".to_owned(),
            goal: goal.to_owned(),
            title: Some("Plant care".to_owned()),
            success_criteria: Vec::new(),
            allowed_tools: Vec::new(),
        }
    }

    /// Submit through the UI path so the in-flight record matches what
    /// production stores, then feed the API failure the async lane delivers.
    fn apply_create_failure(app: &mut StageApp, request: CreateJobRequest, err: &str) {
        app.detail.submitted_job = Some(request);
        app.apply_async_outcome(AsyncOutcome::CreateJob(Err(err.to_owned())));
    }

    #[test]
    fn matching_allow_replays_stashed_job_creation() {
        let mut app = StageApp::new_for_test();
        // Surface the delegate.start ask exactly as the WS event would.
        app.set_pending_approval(PendingApproval {
            id: "approval-1".to_owned(),
            tool: "delegate.start".to_owned(),
            target: "water the plants".to_owned(),
        });
        apply_create_failure(
            &mut app,
            submitted_request("water the plants"),
            "http 403: forbidden: job creation denied",
        );
        assert!(
            app.detail.pending_job_retry.is_some(),
            "an approval-pending rejection must arm the retry stash"
        );
        assert_eq!(
            app.detail
                .pending_job_retry
                .as_ref()
                .map(|retry| retry.approval_id.as_str()),
            Some("approval-1"),
            "stash must already carry the surfaced ask id"
        );

        app.apply_async_outcome(AsyncOutcome::Approval {
            session_id: app.session.session_id().to_owned(),
            allowed: true,
            approval_id: "approval-1".to_owned(),
            result: Ok(()),
        });
        assert_eq!(
            app.detail.pending_job_retry, None,
            "the matching allow consumes the stash"
        );
        app.runtime.block_on(async {
            for _ in 0..200 {
                if app
                    .async_results
                    .lock()
                    .iter()
                    .any(|outcome| matches!(outcome, AsyncOutcome::CreateJob(_)))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        });
        assert!(
            app.async_results
                .lock()
                .iter()
                .any(|outcome| matches!(outcome, AsyncOutcome::CreateJob(_))),
            "the stashed request must be replayed"
        );
    }

    #[test]
    fn deny_is_final_even_for_a_matching_stash() {
        let mut app = StageApp::new_for_test();
        app.detail.pending_job_retry = Some(PendingJobRetry {
            request: submitted_request("water the plants"),
            approval_id: "approval-1".to_owned(),
        });
        app.apply_async_outcome(AsyncOutcome::Approval {
            session_id: app.session.session_id().to_owned(),
            allowed: false,
            approval_id: "approval-1".to_owned(),
            result: Ok(()),
        });
        assert!(
            app.detail.pending_job_retry.is_none(),
            "a deny consumes the stash as the final answer for that ask"
        );
        app.apply_async_outcome(AsyncOutcome::Approval {
            session_id: app.session.session_id().to_owned(),
            allowed: true,
            approval_id: "approval-2".to_owned(),
            result: Ok(()),
        });
        assert!(
            !app.async_results
                .lock()
                .iter()
                .any(|outcome| matches!(outcome, AsyncOutcome::CreateJob(_)))
        );
    }

    #[test]
    fn unrelated_allow_never_replays_the_stash() {
        let mut app = StageApp::new_for_test();
        let request = submitted_request("water the plants");
        app.detail.pending_job_retry = Some(PendingJobRetry {
            approval_id: "approval-1".to_owned(),
            request: request.clone(),
        });
        app.apply_async_outcome(AsyncOutcome::Approval {
            session_id: app.session.session_id().to_owned(),
            allowed: true,
            approval_id: "approval-2".to_owned(),
            result: Ok(()),
        });
        assert_eq!(
            app.detail.pending_job_retry,
            Some(PendingJobRetry {
                request,
                approval_id: "approval-1".to_owned(),
            }),
            "an allow for a different ask must leave the stash untouched"
        );
        assert!(
            !app.async_results
                .lock()
                .iter()
                .any(|outcome| matches!(outcome, AsyncOutcome::CreateJob(_)))
        );
    }

    #[test]
    fn late_delegate_ask_binds_itself_to_the_waiting_stash() {
        let mut app = StageApp::new_for_test();
        apply_create_failure(
            &mut app,
            submitted_request("water the plants"),
            "http 403: forbidden: job creation denied",
        );
        assert!(
            app.detail
                .pending_job_retry
                .as_ref()
                .is_some_and(|retry| retry.approval_id.is_empty()),
            "the stash starts unbound while its ask has not surfaced yet"
        );

        app.set_pending_approval(PendingApproval {
            id: "approval-9".to_owned(),
            tool: "delegate.start".to_owned(),
            target: "water the plants".to_owned(),
        });
        assert_eq!(
            app.detail
                .pending_job_retry
                .as_ref()
                .map(|retry| retry.approval_id.as_str()),
            Some("approval-9"),
            "the matching ask binds its id so its Allow can replay"
        );

        app.apply_async_outcome(AsyncOutcome::Approval {
            session_id: app.session.session_id().to_owned(),
            allowed: true,
            approval_id: "approval-9".to_owned(),
            result: Ok(()),
        });
        assert_eq!(app.detail.pending_job_retry, None);
    }

    #[test]
    fn only_approval_failures_are_stashed_for_retry() {
        let mut app = StageApp::new_for_test();
        let request = submitted_request("network blip");
        apply_create_failure(
            &mut app,
            request.clone(),
            "http 403: forbidden: job creation denied",
        );
        assert_eq!(
            app.detail
                .pending_job_retry
                .as_ref()
                .map(|retry| &retry.request),
            Some(&request),
            "an approval-pending failure is exactly what a later allow should retry"
        );
        apply_create_failure(&mut app, request.clone(), "http 500: internal error");
        assert_eq!(
            app.detail.pending_job_retry, None,
            "a non-approval failure must not be replayable by a later allow"
        );
    }

    #[test]
    fn repaints_after_input_even_when_the_flag_is_dropped() {
        assert!(!should_repaint_after_event(false, false));

        // Typing and pasting raise the Slint redraw flag; honoring it
        // keeps their frame immediate even while an OS throttles the loop.
        assert!(should_repaint_after_event(true, false));
    }

    #[test]
    fn composer_focus_forces_repaint_on_every_chat_event() {
        // Trailing mouse moves never set the flag; a focused composer still
        // needs every event to end in a redraw request (request_redraw
        // coalesces internally, so this stays cheap).
        assert!(should_repaint_after_event(false, true));
        assert!(should_repaint_after_event(true, true));
    }

    #[test]
    fn detail_drafts_are_not_pushed_while_the_same_tab_is_focused() {
        assert!(!should_push_detail_drafts(true, 2, 2));
        assert!(should_push_detail_drafts(true, 2, 3));
        assert!(should_push_detail_drafts(false, 2, 2));
    }

    #[test]
    fn question_answer_prefers_the_question_field() {
        assert_eq!(
            question_answer_text(" typed answer ", "composer", "prompt"),
            "typed answer"
        );
        assert_eq!(question_answer_text("  ", "composer", "prompt"), "composer");
        assert_eq!(question_answer_text("", "", "prompt"), "prompt");
    }

    #[test]
    fn message_mode_index_round_trips() {
        assert_eq!(message_mode_from_index(0), MessageMode::Prompt);
        assert_eq!(message_mode_from_index(1), MessageMode::Steer);
        assert_eq!(message_mode_from_index(2), MessageMode::FollowUp);
        assert_eq!(message_mode_from_index(99), MessageMode::Prompt);
    }
}

#[test]
fn request_active_soul_enqueues_a_load_soul_outcome() {
    let mut app = StageApp::new_for_test();
    assert!(app.detail.soul.is_none());
    app.request_active_soul();
    // The outcome is produced asynchronously; let the spawned task settle.
    app.runtime.block_on(async {
        for _ in 0..200 {
            if app
                .async_results
                .lock()
                .iter()
                .any(|outcome| matches!(outcome, AsyncOutcome::LoadSoul(_)))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    });
    let queued = app
        .async_results
        .lock()
        .iter()
        .any(|outcome| matches!(outcome, AsyncOutcome::LoadSoul(_)));
    assert!(
        queued,
        "boot should request the active soul so Home readiness is correct without opening Companion"
    );
}

#[test]
fn mic_toggle_is_blocked_until_stt_is_configured() {
    let mut app = StageApp::new_for_test();
    assert!(!app.mic_active);
    // No STT provider yet: turning the mic on must not claim it.
    app.toggle_mic();
    assert!(!app.mic_active, "mic must stay off without STT");
    assert!(
        !app.surface.status.is_empty(),
        "a Voice-setup CTA should be surfaced"
    );
    assert!(
        app.surface.stt_setup_needed,
        "the mic guard must arm the dedicated STT setup flag"
    );

    // Once a real STT provider is observed in effective settings, the
    // settings-load path recomputes the ready mirror via parse_core_fields;
    // the toggle reads that flag rather than re-parsing plugin strings.
    detail::parse_core_fields(
        r#"{"effective": {"ai": {"tasks": {"stt": {"plugin": "whisper.cpp"}}}}}"#,
        &mut app.detail,
    );
    // Same disarm path the settings-load callback uses.
    app.sync_stt_cta_after_settings_parse();
    // Clear the prior CTA so we can confirm the configured path does not
    // re-surface it.
    app.surface.status.clear();
    app.toggle_mic();
    // The synchronous guard must not surface the CTA when STT is
    // configured; the actual claim is performed asynchronously.
    assert!(
        app.surface.status.is_empty(),
        "configured STT must not trigger the STT-missing CTA"
    );
    assert!(
        !app.surface.stt_setup_needed,
        "configured STT must leave any earlier STT-missing CTA unarmed"
    );
}
