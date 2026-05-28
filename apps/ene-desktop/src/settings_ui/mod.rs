mod page_ai;
mod page_character;
mod page_graphics;
mod widgets;

use page_ai::render_ai_page;
use page_character::render_character_page;
use page_graphics::render_graphics_page;

use crate::ai_bridge::{EneRequestEvent, EneStreamEvent};
use crate::app_config::{
    AntialiasingMode, CharacterSettings, SETTINGS_WINDOW_HEIGHT, SETTINGS_WINDOW_WIDTH,
    ShadowQuality, target_fps_label,
};
use crate::character::{CharacterAnimationControl, EmotionQueue};
use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::window::{WindowRef, WindowResolution};
use bevy_egui::{EguiContext, EguiMultipassSchedule, PrimaryEguiContext, egui};
use ene_core::{EmbeddingConfig, MemoryConfig};
use widgets::apply_action;

#[derive(bevy::ecs::schedule::ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct SettingsWindowContextPass;

pub struct SettingsUiPlugin;

impl Plugin for SettingsUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SettingsInputState>()
            .init_resource::<SettingsWindowEntities>()
            .add_systems(
                Update,
                (
                    toggle_settings_visibility_shortcut,
                    handle_settings_keyboard_controls,
                    apply_ai_stream_events,
                    apply_settings_window_visibility,
                    handle_settings_window_close,
                )
                    .chain(),
            )
            .add_systems(SettingsWindowContextPass, render_settings_window);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsPageKind {
    Character,
    Graphics,
    Ai,
}

impl SettingsPageKind {
    fn label(self) -> &'static str {
        match self {
            SettingsPageKind::Character => "Character",
            SettingsPageKind::Graphics => "Graphics",
            SettingsPageKind::Ai => "AI",
        }
    }
}

#[derive(Resource, Debug)]
struct SettingsInputState {
    current_page: SettingsPageKind,
    look_at_strength: String,
    model_scale: String,
    character_pos_x: String,
    character_pos_y: String,
    character_pos_z: String,
    ai_user_name: String,
    ai_runtime_rules: String,
    ai_base_url: String,
    ai_api_key: String,
    ai_chat_input: String,
    ai_memory_enabled: bool,
    ai_embedding_provider: String,
    ai_embedding_model: String,
    ai_embedding_dimensions: String,
    ai_embedding_base_url: String,
}

#[derive(Resource, Default, Debug)]
struct SettingsWindowEntities {
    window: Option<Entity>,
    camera: Option<Entity>,
}

impl Default for SettingsInputState {
    fn default() -> Self {
        Self {
            current_page: SettingsPageKind::Character,
            look_at_strength: String::new(),
            model_scale: String::new(),
            character_pos_x: String::new(),
            character_pos_y: String::new(),
            character_pos_z: String::new(),
            ai_user_name: String::new(),
            ai_runtime_rules: String::new(),
            ai_base_url: String::new(),
            ai_api_key: String::new(),
            ai_chat_input: String::new(),
            ai_memory_enabled: false,
            ai_embedding_provider: "local".to_string(),
            ai_embedding_model: "jina-embeddings-v5-text-small".to_string(),
            ai_embedding_dimensions: "auto".to_string(),
            ai_embedding_base_url: String::new(),
        }
    }
}

impl SettingsInputState {
    fn sync_from_settings(&mut self, settings: &CharacterSettings) {
        self.look_at_strength = format!("{:.2}", settings.character_state.look_at_strength);
        self.model_scale = format!("{:.2}", settings.character_state.model_scale);
        self.character_pos_x = format!("{:+.2}", settings.character_state.character_position.x);
        self.character_pos_y = format!("{:+.2}", settings.character_state.character_position.y);
        self.character_pos_z = format!("{:+.2}", settings.character_state.character_position.z);
        self.ai_user_name = settings.ai.ai.user_name.clone();
        self.ai_runtime_rules = settings.ai.ai.runtime_rules.clone();
        let provider_config = settings
            .ai
            .ai
            .get_section::<ene_core::ProviderConfig>()
            .unwrap_or_default();
        self.ai_base_url = provider_config.base_url.clone();
        self.ai_api_key = provider_config.api_key.clone();
        self.ai_chat_input = settings.ui.ai_chat_input.clone();
        let mem_config = settings
            .ai
            .ai
            .get_section::<MemoryConfig>()
            .unwrap_or_default();
        let embed_config = settings
            .ai
            .ai
            .get_section::<EmbeddingConfig>()
            .unwrap_or_default();

        self.ai_memory_enabled = mem_config.enabled;
        self.ai_embedding_provider = match embed_config.provider_type {
            ene_core::EmbeddingProviderType::Api => "api".to_string(),
            ene_core::EmbeddingProviderType::Local => "local".to_string(),
        };
        self.ai_embedding_model = embed_config.model.clone();
        self.ai_embedding_base_url = embed_config.base_url.clone();
        self.ai_embedding_dimensions = embed_config
            .dimensions
            .map(|d| d.to_string())
            .unwrap_or_else(|| "auto".to_string());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsValueKind {
    Character,
    Motion,
    AnimationState,
    DebugOverlay,
    MaskRenderDownsample,
    TargetFps,
    ShadowQuality,
    AntialiasingMode,
    LookAtStrength,
    ModelScale,
    CharacterPositionX,
    CharacterPositionY,
    CharacterPositionZ,
    AiUserName,
    AiRuntimeRules,
    AiProviderName,
    AiModel,
    AiBaseUrl,
    AiApiKey,
    AiChatInput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsButtonAction {
    PrevCharacter,
    NextCharacter,
    PrevMotion,
    NextMotion,
    TogglePlay,
    ToggleDebugOverlay,
    MaskDownsampleDown,
    MaskDownsampleUp,
    TargetFpsDown,
    TargetFpsUp,
    ShadowQualityDown,
    ShadowQualityUp,
    AntialiasingModeDown,
    AntialiasingModeUp,
    LookAtStrengthDown,
    LookAtStrengthUp,
    ModelScaleDown,
    ModelScaleUp,
    CharacterPosXDown,
    CharacterPosXUp,
    CharacterPosYDown,
    CharacterPosYUp,
    CharacterPosZDown,
    CharacterPosZUp,
    SendAiChat,
}

impl SettingsValueKind {
    fn current_text(
        self,
        settings: &CharacterSettings,
        animation_control: &CharacterAnimationControl,
    ) -> String {
        match self {
            SettingsValueKind::Character => format!(
                "[{}/{}] {}",
                settings.character_state.selected_character + 1,
                settings.characters.len(),
                settings.current_entry().name
            ),
            SettingsValueKind::Motion => format!(
                "[{}/{}] {}",
                settings.character_state.selected_motion + 1,
                settings.current_entry().motion_paths.len(),
                compact_asset_name(settings.current_motion())
            ),
            SettingsValueKind::AnimationState => {
                if animation_control.playing {
                    "Playing".to_string()
                } else {
                    "Paused".to_string()
                }
            }
            SettingsValueKind::DebugOverlay => {
                if settings.ui.debug_overlay_visible {
                    "Visible".to_string()
                } else {
                    "Hidden".to_string()
                }
            }
            SettingsValueKind::MaskRenderDownsample => {
                format!("{}x", settings.graphics.mask_render_downsample)
            }
            SettingsValueKind::TargetFps => target_fps_label(settings.graphics.target_fps),
            SettingsValueKind::ShadowQuality => match settings.graphics.shadow_quality {
                ShadowQuality::Low => "Low",
                ShadowQuality::Medium => "Medium",
                ShadowQuality::High => "High",
            }
            .to_string(),
            SettingsValueKind::AntialiasingMode => match settings.graphics.antialiasing_mode {
                AntialiasingMode::Off => "Off",
                AntialiasingMode::Fxaa => "Fxaa",
                AntialiasingMode::Smaa => "Smaa",
                AntialiasingMode::Taa => "Taa",
            }
            .to_string(),
            SettingsValueKind::LookAtStrength => {
                format!("{:.2}", settings.character_state.look_at_strength)
            }
            SettingsValueKind::ModelScale => format!("{:.2}", settings.character_state.model_scale),
            SettingsValueKind::CharacterPositionX => {
                format!("{:+.2}", settings.character_state.character_position.x)
            }
            SettingsValueKind::CharacterPositionY => {
                format!("{:+.2}", settings.character_state.character_position.y)
            }
            SettingsValueKind::CharacterPositionZ => {
                format!("{:+.2}", settings.character_state.character_position.z)
            }
            SettingsValueKind::AiUserName => settings.ai.ai.user_name.clone(),
            SettingsValueKind::AiRuntimeRules => settings.ai.ai.runtime_rules.clone(),
            SettingsValueKind::AiProviderName => {
                let provider_config = settings
                    .ai
                    .ai
                    .get_section::<ene_core::ProviderConfig>()
                    .unwrap_or_default();
                provider_config.provider_name.clone()
            }
            SettingsValueKind::AiModel => {
                let provider_config = settings
                    .ai
                    .ai
                    .get_section::<ene_core::ProviderConfig>()
                    .unwrap_or_default();
                provider_config.model.clone()
            }
            SettingsValueKind::AiBaseUrl => {
                let provider_config = settings
                    .ai
                    .ai
                    .get_section::<ene_core::ProviderConfig>()
                    .unwrap_or_default();
                provider_config.base_url.clone()
            }
            SettingsValueKind::AiApiKey => {
                let provider_config = settings
                    .ai
                    .ai
                    .get_section::<ene_core::ProviderConfig>()
                    .unwrap_or_default();
                masked_secret(&provider_config.api_key)
            }
            SettingsValueKind::AiChatInput => settings.ui.ai_chat_input.clone(),
        }
    }

    fn apply_input(self, value: &str, settings: &mut CharacterSettings) -> Result<(), ()> {
        match self {
            SettingsValueKind::LookAtStrength => parse_and_assign(value, |parsed| {
                settings.character_state.look_at_strength = parsed
            }),
            SettingsValueKind::ModelScale => parse_and_assign(value, |parsed| {
                settings.character_state.model_scale = parsed
            }),
            SettingsValueKind::CharacterPositionX => parse_and_assign(value, |parsed| {
                settings.character_state.character_position.x = parsed
            }),
            SettingsValueKind::CharacterPositionY => parse_and_assign(value, |parsed| {
                settings.character_state.character_position.y = parsed
            }),
            SettingsValueKind::CharacterPositionZ => parse_and_assign(value, |parsed| {
                settings.character_state.character_position.z = parsed
            }),
            SettingsValueKind::AiUserName => {
                settings.ai.ai.user_name = value.to_string();
                Ok(())
            }
            SettingsValueKind::AiRuntimeRules => {
                settings.ai.ai.runtime_rules = value.to_string();
                Ok(())
            }
            SettingsValueKind::AiBaseUrl => {
                let mut provider_config = settings
                    .ai
                    .ai
                    .get_section::<ene_core::ProviderConfig>()
                    .unwrap_or_default();
                provider_config.base_url = value.to_string();
                let _ = settings.ai.ai.set_section(&provider_config);
                Ok(())
            }
            SettingsValueKind::AiApiKey => {
                let mut provider_config = settings
                    .ai
                    .ai
                    .get_section::<ene_core::ProviderConfig>()
                    .unwrap_or_default();
                if provider_config.api_key_source == "keyring" {
                    let service = &provider_config.api_key_keyring_service;
                    let account = &provider_config.api_key_keyring_account;
                    match keyring::Entry::new(service, account) {
                        Ok(entry) => {
                            if let Err(e) = entry.set_password(value) {
                                error!("[Security] Failed to write API key to keyring: {}", e);
                            } else {
                                info!("[Security] API key stored in keyring.");
                                provider_config.api_key = String::new();
                            }
                        }
                        Err(e) => {
                            error!("[Security] Failed to initialize keyring: {}", e);
                        }
                    }
                } else {
                    provider_config.api_key = value.to_string();
                }
                let _ = settings.ai.ai.set_section(&provider_config);
                Ok(())
            }
            SettingsValueKind::AiChatInput => {
                settings.ui.ai_chat_input = value.to_string();
                Ok(())
            }
            SettingsValueKind::Character
            | SettingsValueKind::Motion
            | SettingsValueKind::AnimationState
            | SettingsValueKind::DebugOverlay
            | SettingsValueKind::MaskRenderDownsample
            | SettingsValueKind::TargetFps
            | SettingsValueKind::ShadowQuality
            | SettingsValueKind::AntialiasingMode
            | SettingsValueKind::AiProviderName
            | SettingsValueKind::AiModel => Err(()),
        }
    }
}

fn parse_and_assign<T>(value: &str, assign: impl FnOnce(T)) -> Result<(), ()>
where
    T: std::str::FromStr,
{
    value.parse::<T>().map(assign).map_err(|_| ())
}

fn toggle_settings_visibility_shortcut(
    keys: Res<ButtonInput<KeyCode>>,
    mut settings: ResMut<CharacterSettings>,
    mut input_state: ResMut<SettingsInputState>,
) {
    if keys.just_pressed(KeyCode::F1) {
        settings.ui.settings_window_visible = !settings.ui.settings_window_visible;
        if settings.ui.settings_window_visible {
            input_state.sync_from_settings(&settings);
        } else {
            settings.save();
        }
    }
}

fn handle_settings_keyboard_controls(
    keys: Res<ButtonInput<KeyCode>>,
    egui_ctx: Option<Single<&mut EguiContext, Without<PrimaryEguiContext>>>,
    mut settings: ResMut<CharacterSettings>,
    mut animation_control: ResMut<CharacterAnimationControl>,
    mut ai_request_writer: MessageWriter<EneRequestEvent>,
    _window_entities: Res<SettingsWindowEntities>,
    input_state: Res<SettingsInputState>,
) {
    if !settings.ui.settings_window_visible {
        return;
    }

    if keys.just_pressed(KeyCode::Escape) {
        settings.ui.settings_window_visible = false;
        settings.save();
        return;
    }

    let egui_has_focus = egui_ctx
        .map(|mut ctx| ctx.get_mut().wants_keyboard_input())
        .unwrap_or(false);

    if egui_has_focus || input_state.current_page != SettingsPageKind::Character {
        return;
    }

    for (key, action) in [
        (KeyCode::KeyA, SettingsButtonAction::PrevCharacter),
        (KeyCode::KeyD, SettingsButtonAction::NextCharacter),
        (KeyCode::KeyW, SettingsButtonAction::PrevMotion),
        (KeyCode::KeyS, SettingsButtonAction::NextMotion),
        (KeyCode::Space, SettingsButtonAction::TogglePlay),
    ] {
        if keys.just_pressed(key) {
            apply_action(
                action,
                &mut settings,
                &mut animation_control,
                &mut ai_request_writer,
            );
        }
    }
}

fn render_settings_window(
    egui_ctx: Option<Single<&mut EguiContext, Without<PrimaryEguiContext>>>,
    mut settings: ResMut<CharacterSettings>,
    mut animation_control: ResMut<CharacterAnimationControl>,
    mut ai_request_writer: MessageWriter<EneRequestEvent>,
    mut input_state: ResMut<SettingsInputState>,
    mut emotion_queue: ResMut<EmotionQueue>,
    time: Res<Time>,
) {
    if !settings.ui.settings_window_visible {
        return;
    }

    let Some(mut ctx_single) = egui_ctx else {
        return;
    };

    let ctx = ctx_single.get_mut();

    apply_egui_visuals(ctx);

    egui::CentralPanel::default().show(ctx, |ui| {
        ui.horizontal(|ui| {
            for page in [
                SettingsPageKind::Character,
                SettingsPageKind::Graphics,
                SettingsPageKind::Ai,
            ] {
                if ui
                    .selectable_label(input_state.current_page == page, page.label())
                    .clicked()
                {
                    input_state.current_page = page;
                }
            }
        });
        ui.separator();
        ui.add_space(8.0);

        match input_state.current_page {
            SettingsPageKind::Character => {
                render_character_page(
                    ui,
                    &mut settings,
                    &mut animation_control,
                    &mut ai_request_writer,
                    &mut input_state,
                    &mut emotion_queue,
                    &time,
                );
            }
            SettingsPageKind::Graphics => {
                render_graphics_page(
                    ui,
                    &mut settings,
                    &mut animation_control,
                    &mut ai_request_writer,
                );
            }
            SettingsPageKind::Ai => {
                render_ai_page(
                    ui,
                    &mut settings,
                    &mut animation_control,
                    &mut ai_request_writer,
                    &mut input_state,
                );
            }
        }

        ui.add_space(8.0);
        ui.separator();
        ui.small("F1: Open/Close  |  Esc: Hide  |  A/D,W/S: Char/Motion");
    });

    if let Some(req) = settings.ui.pending_permission.clone() {
        let mut open = true;
        egui::Window::new("⚠️ 承認の要求 / Permission Required")
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.heading("破壊的操作の許可を求めています");
                    ui.add_space(8.0);
                });

                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.strong("アクション: ");
                        ui.label(&req.action);
                    });
                    ui.horizontal(|ui| {
                        ui.strong("対象: ");
                        ui.label(&req.target);
                    });
                    ui.horizontal(|ui| {
                        ui.strong("詳細: ");
                        ui.label(&req.description);
                    });
                });

                ui.add_space(12.0);

                ui.horizontal(|ui| {
                    ui.columns(3, |columns| {
                        if columns[0].button("1回のみ許可\n(Allow Once)").clicked() {
                            let _ = ene_core::stream::submit_permission_decision(
                                &req.request_id,
                                ene_core::stream::PermissionDecision::AllowOnce,
                            );
                            settings.ui.pending_permission = None;
                        }
                        if columns[1]
                            .button("セッションで許可\n(Allow Session)")
                            .clicked()
                        {
                            let _ = ene_core::stream::submit_permission_decision(
                                &req.request_id,
                                ene_core::stream::PermissionDecision::AllowSession,
                            );
                            settings.ui.pending_permission = None;
                        }
                        if columns[2].button("拒否\n(Deny)").clicked() {
                            let _ = ene_core::stream::submit_permission_decision(
                                &req.request_id,
                                ene_core::stream::PermissionDecision::Deny,
                            );
                            settings.ui.pending_permission = None;
                        }
                    });
                });
                ui.add_space(8.0);
            });

        if !open {
            let _ = ene_core::stream::submit_permission_decision(
                &req.request_id,
                ene_core::stream::PermissionDecision::Deny,
            );
            settings.ui.pending_permission = None;
        }
    }
}

fn apply_settings_window_visibility(
    settings: Res<CharacterSettings>,
    mut commands: Commands,
    mut window_entities: ResMut<SettingsWindowEntities>,
    mut input_state: ResMut<SettingsInputState>,
    mut windows: Query<&mut Window>,
) {
    if settings.ui.settings_window_visible {
        if window_entities.window.is_none() {
            let window = commands
                .spawn((Window {
                    title: "ene Settings".to_string(),
                    resolution: WindowResolution::new(
                        SETTINGS_WINDOW_WIDTH,
                        SETTINGS_WINDOW_HEIGHT,
                    ),
                    ime_enabled: true,
                    ..default()
                },))
                .id();

            let camera = commands
                .spawn((
                    Camera2d,
                    Camera {
                        is_active: true,
                        ..default()
                    },
                    RenderTarget::Window(WindowRef::Entity(window)),
                    EguiMultipassSchedule::new(SettingsWindowContextPass),
                ))
                .id();

            window_entities.window = Some(window);
            window_entities.camera = Some(camera);
            input_state.sync_from_settings(&settings);
        }

        if let Some(window_entity) = window_entities.window
            && let Ok(mut window) = windows.get_mut(window_entity)
        {
            window.visible = true;
        }

        return;
    }

    if let Some(camera_entity) = window_entities.camera.take() {
        commands.entity(camera_entity).despawn();
    }
    if let Some(window_entity) = window_entities.window.take() {
        commands.entity(window_entity).despawn();
    }
}

fn apply_ai_stream_events(
    mut stream_events: MessageReader<EneStreamEvent>,
    mut settings: ResMut<CharacterSettings>,
) {
    for event in stream_events.read() {
        match event {
            EneStreamEvent::TextDelta(delta) => {
                settings.ui.ai_latest_response.push_str(delta);
            }
            EneStreamEvent::Finished => {}
            EneStreamEvent::Error(error) => {
                if !settings.ui.ai_latest_response.is_empty() {
                    settings.ui.ai_latest_response.push('\n');
                }
                settings.ui.ai_latest_response.push_str("[error] ");
                settings.ui.ai_latest_response.push_str(error);
            }
            EneStreamEvent::SpecialToken(_) => {}
            EneStreamEvent::ToolCallStart { .. } => {}
            EneStreamEvent::ToolCallResult { .. } => {}
            EneStreamEvent::PermissionRequired {
                request_id,
                action,
                target,
                description,
            } => {
                settings.ui.pending_permission = Some(crate::app_config::PendingPermission {
                    request_id: request_id.clone(),
                    action: action.clone(),
                    target: target.clone(),
                    description: description.clone(),
                });
                settings.ui.settings_window_visible = true;
            }
            EneStreamEvent::TaskProgress { .. } => {}
        }
    }
}

fn apply_egui_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = egui::Color32::from_rgb(26, 28, 33);
    visuals.window_fill = egui::Color32::from_rgb(20, 22, 28);
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(30, 33, 38);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(38, 42, 50);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(52, 57, 66);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(72, 77, 89);
    visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(220, 224, 232);
    visuals.widgets.hovered.fg_stroke.color = egui::Color32::from_rgb(240, 243, 248);
    visuals.widgets.active.fg_stroke.color = egui::Color32::from_rgb(247, 248, 250);
    ctx.set_visuals(visuals);
}

fn compact_asset_name(path: &str) -> String {
    if path.len() <= 30 {
        return path.to_string();
    }
    format!("...{}", &path[path.len() - 27..])
}

fn masked_secret(value: &str) -> String {
    if value.trim().is_empty() {
        "(empty)".to_string()
    } else {
        "********".to_string()
    }
}

fn handle_settings_window_close(
    mut events: MessageReader<bevy::window::WindowCloseRequested>,
    window_entities: Res<SettingsWindowEntities>,
    mut settings: ResMut<CharacterSettings>,
) {
    for event in events.read() {
        if Some(event.window) == window_entities.window {
            settings.ui.settings_window_visible = false;
            settings.save();
        }
    }
}
