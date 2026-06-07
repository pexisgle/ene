use bevy::prelude::*;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use bevy::window::CompositeAlphaMode;
#[cfg(target_os = "windows")]
use bevy::window::PresentMode;
use bevy::window::{WindowLevel, WindowMode, WindowPlugin, WindowResolution};
use ene_core::RequestId;
use ene_tool_proto::UserInputPrompt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

ene_config::define_label_enum!(
    pub enum ShadowQuality {
        Low => "Low" => 1_024,
        #[default]
        Medium => "Medium" => 2_048,
        High => "High" => 4_096,
    }
    [shadow_map_size: usize]
);

ene_config::define_label_enum!(
    pub enum AntialiasingMode {
        Off => "Off",
        #[default]
        Fxaa => "FXAA",
        Smaa => "SMAA",
        Taa => "TAA",
    }
);

ene_config::define_config!(
    "graphics",
    pub struct GraphicsSection {
        pub mask_render_downsample: u32 = 8,
        pub target_fps: u32 = 60,
        pub shadow_quality: ShadowQuality = ShadowQuality::Medium,
        pub antialiasing_mode: AntialiasingMode = AntialiasingMode::Fxaa,
    }
);

#[derive(Debug, Default, Clone, Serialize, Deserialize, ene_config::schemars::JsonSchema)]
#[schemars(crate = "::ene_config::schemars")]
pub struct DesktopSection {
    pub graphics: GraphicsSection,
}

impl ene_config::HasConfigKey for DesktopSection {
    const KEY: &'static str = "desktop";
}

pub const DEFAULT_CHARACTER_NAME: &str = "Alicia";
pub const DEFAULT_VRM_PATH: &str = "characters/Alicia/AliciaSolid.vrm";
pub const DEFAULT_VRMA_PATH: &str = "characters/Alicia/motions/VRMA_01.vrma";
pub const APP_ID: &str = "dev.pexisgle.ene";
pub const WINDOW_WIDTH: u32 = 560;
pub const WINDOW_HEIGHT: u32 = 980;
pub const SETTINGS_WINDOW_WIDTH: u32 = 460;
pub const SETTINGS_WINDOW_HEIGHT: u32 = 620;
pub const MASK_RENDER_DOWNSAMPLE_CHOICES: [u32; 3] = [4, 6, 8];
pub const DEFAULT_MASK_RENDER_DOWNSAMPLE: u32 = 8;
pub const TARGET_FPS_CHOICES: [u32; 5] = [15, 30, 60, 120, 0];
pub const DEFAULT_TARGET_FPS: u32 = 60;
pub const SHADOW_QUALITY_CHOICES: [ShadowQuality; 3] = [
    ShadowQuality::Low,
    ShadowQuality::Medium,
    ShadowQuality::High,
];
pub const DEFAULT_SHADOW_QUALITY: ShadowQuality = ShadowQuality::Medium;
pub const ANTIALIASING_MODE_CHOICES: [AntialiasingMode; 4] = [
    AntialiasingMode::Off,
    AntialiasingMode::Fxaa,
    AntialiasingMode::Smaa,
    AntialiasingMode::Taa,
];
pub const DEFAULT_ANTIALIASING_MODE: AntialiasingMode = AntialiasingMode::Fxaa;

pub fn normalize_mask_render_downsample(value: u32) -> u32 {
    cycle_choice(
        &MASK_RENDER_DOWNSAMPLE_CHOICES,
        value,
        0,
        DEFAULT_MASK_RENDER_DOWNSAMPLE,
    )
}

pub fn cycle_mask_render_downsample(current: u32, step: isize) -> u32 {
    cycle_choice(
        &MASK_RENDER_DOWNSAMPLE_CHOICES,
        current,
        step,
        DEFAULT_MASK_RENDER_DOWNSAMPLE,
    )
}

pub fn normalize_target_fps(value: u32) -> u32 {
    cycle_choice(&TARGET_FPS_CHOICES, value, 0, DEFAULT_TARGET_FPS)
}

pub fn cycle_target_fps(current: u32, step: isize) -> u32 {
    cycle_choice(&TARGET_FPS_CHOICES, current, step, DEFAULT_TARGET_FPS)
}

pub fn cycle_shadow_quality(current: ShadowQuality, step: isize) -> ShadowQuality {
    cycle_choice(
        &SHADOW_QUALITY_CHOICES,
        current,
        step,
        DEFAULT_SHADOW_QUALITY,
    )
}

pub fn cycle_antialiasing_mode(current: AntialiasingMode, step: isize) -> AntialiasingMode {
    cycle_choice(
        &ANTIALIASING_MODE_CHOICES,
        current,
        step,
        DEFAULT_ANTIALIASING_MODE,
    )
}

fn cycle_choice<T: Copy + PartialEq>(choices: &[T], current: T, step: isize, _default: T) -> T {
    let index = choices
        .iter()
        .position(|candidate| *candidate == current)
        .unwrap_or(1);
    let len = choices.len() as isize;
    let next_index = (index as isize + step).rem_euclid(len) as usize;
    choices[next_index]
}

pub fn target_fps_label(target_fps: u32) -> String {
    if target_fps == 0 {
        "Unlimited".to_string()
    } else {
        format!("{target_fps} FPS")
    }
}

/// Reads optional VRM and VRMA overrides from the first two CLI arguments.
pub fn read_cli_paths() -> (String, String) {
    let vrm = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_VRM_PATH.to_string());
    let vrma = std::env::args()
        .nth(2)
        .unwrap_or_else(|| DEFAULT_VRMA_PATH.to_string());
    (vrm, vrma)
}

/// Builds the transparent always-on-top main window used by the character view.
pub fn window_plugin() -> WindowPlugin {
    WindowPlugin {
        primary_window: Some(Window {
            title: "ene".to_string(),
            name: Some(APP_ID.to_string()),
            resolution: WindowResolution::new(WINDOW_WIDTH, WINDOW_HEIGHT),
            resizable: false,
            decorations: false,
            transparent: true,
            #[cfg(target_os = "windows")]
            mode: WindowMode::Windowed,
            #[cfg(target_os = "windows")]
            present_mode: PresentMode::Mailbox,
            #[cfg(not(target_os = "windows"))]
            mode: WindowMode::BorderlessFullscreen(MonitorSelection::Primary),
            #[cfg(target_os = "macos")]
            composite_alpha_mode: CompositeAlphaMode::PostMultiplied,
            #[cfg(target_os = "linux")]
            composite_alpha_mode: CompositeAlphaMode::PreMultiplied,
            window_level: WindowLevel::AlwaysOnTop,
            ..default()
        }),
        ..default()
    }
}

#[derive(Debug, Clone)]
pub struct CharacterEntry {
    pub name: String,
    #[expect(dead_code)]
    pub folder: String,
    pub vrm_paths: Vec<String>,
    pub motion_paths: Vec<String>,
    pub card_path: String,
    pub default_motion: Option<String>,
}

pub use GraphicsSection as GraphicsSettings;

#[derive(Clone, Debug)]
pub struct CharacterState {
    pub selected_character: usize,
    pub selected_motion: usize,
    pub needs_respawn: bool,
    pub model_scale: f32,
    pub character_position: Vec3,
    pub look_at_strength: f32,
}

impl Default for CharacterState {
    fn default() -> Self {
        Self {
            selected_character: 0,
            selected_motion: 0,
            needs_respawn: true,
            model_scale: 1.0,
            character_position: Vec3::ZERO,
            look_at_strength: 0.60,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingPermission {
    pub request_id: RequestId,
    pub action: String,
    pub target: String,
    pub description: String,
}

/// A pending interactive user input request surfaced by an interactive tool
/// (e.g. the `question` tool). The UI is responsible for collecting a
/// response and calling `EneHandle::submit_user_input`.
#[derive(Clone, Debug)]
pub struct PendingUserInput {
    pub request_id: RequestId,
    pub prompt: UserInputPrompt,
}

/// In-progress user answer for a single sub-question. The UI updates these
/// fields as the user interacts and only the final `Multi(Vec<MultiAnswer>)`
/// is sent to the actor on submit.
#[derive(Clone, Debug, Default)]
pub struct QuestionDraft {
    /// Selected option (only used when the question declares `options`).
    pub selected: Option<String>,
    /// Free-text input (only used when `allow_free_text` is true).
    pub text: String,
    /// Whether the user marked this question as skipped.
    pub skipped: bool,
}

#[derive(Clone, Debug, Default)]
pub struct UiState {
    pub settings_window_visible: bool,
    #[allow(dead_code)]
    pub debug_overlay_visible: bool,
    pub ai_chat_input: String,
    pub ai_latest_response: String,
    pub pending_permission: Option<PendingPermission>,
    /// Interactive question dialog state. `Some` when the actor is paused
    /// waiting for a user response.
    pub pending_user_input: Option<PendingUserInput>,
    /// Per-question draft answers, parallel to `pending_user_input.prompt.items`.
    /// `None` while no prompt is active. Index aligns with `items[i]`.
    pub user_input_drafts: Vec<QuestionDraft>,
}

#[derive(Clone, Debug, Default)]
pub struct AiConfig {
    pub ai: ene_config::EneConfig,
}

#[derive(Resource, Debug)]
pub struct CharacterSettings {
    pub assets_dir: PathBuf,
    pub characters: Vec<CharacterEntry>,
    pub graphics: GraphicsSettings,
    pub character_state: CharacterState,
    pub ui: UiState,
    pub ai: AiConfig,
}

impl CharacterSettings {
    pub fn discover(assets_dir: &Path, default_vrm: String) -> Self {
        let mut characters = discover_characters(assets_dir);

        if characters.is_empty() {
            characters.push(CharacterEntry {
                name: DEFAULT_CHARACTER_NAME.to_string(),
                folder: DEFAULT_CHARACTER_NAME.to_string(),
                vrm_paths: vec![DEFAULT_VRM_PATH.to_string()],
                motion_paths: vec![DEFAULT_VRMA_PATH.to_string()],
                card_path: format!("characters/{DEFAULT_CHARACTER_NAME}/character.json"),
                default_motion: None,
            });
        }

        let selected_character = characters
            .iter()
            .position(|c| c.vrm_paths.iter().any(|v| v == &default_vrm))
            .unwrap_or(0);

        let default_card_path = format!("characters/{DEFAULT_CHARACTER_NAME}/character.json");
        let selected_card_path = characters
            .get(selected_character)
            .map_or(default_card_path, |c| c.card_path.clone());
        let selected_motion = characters
            .get(selected_character)
            .and_then(|entry| {
                entry
                    .default_motion
                    .as_ref()
                    .and_then(|dm| entry.motion_paths.iter().position(|m| m.ends_with(dm)))
            })
            .unwrap_or(0);

        let mut settings = Self {
            assets_dir: assets_dir.to_path_buf(),
            characters,
            graphics: GraphicsSettings::default(),
            character_state: CharacterState {
                selected_character,
                selected_motion,
                ..Default::default()
            },
            ui: UiState::default(),
            ai: AiConfig {
                ai: ene_config::EneConfig {
                    character: format!("{}/{}", assets_dir.display(), selected_card_path),
                    ..Default::default()
                },
            },
        };
        settings.load_from_file();
        settings
    }

    pub fn current_entry(&self) -> &CharacterEntry {
        &self.characters[self.character_state.selected_character]
    }

    pub fn current_character(&self) -> &str {
        &self.current_entry().vrm_paths[0]
    }

    pub fn current_motion(&self) -> &str {
        &self.current_entry().motion_paths[self.character_state.selected_motion]
    }

    pub fn current_character_card(&self) -> &str {
        &self.current_entry().card_path
    }

    pub fn sync_card_path(&mut self) {
        let path = format!(
            "{}/{}",
            self.assets_dir.display(),
            self.current_character_card()
        );
        self.ai.ai.character = path;
    }

    pub fn clamp_runtime_values(&mut self) {
        self.character_state.model_scale = self.character_state.model_scale.clamp(0.25, 4.0);
        self.character_state.character_position.x =
            self.character_state.character_position.x.clamp(-3.0, 3.0);
        self.character_state.character_position.y =
            self.character_state.character_position.y.clamp(-2.0, 3.0);
        self.character_state.character_position.z =
            self.character_state.character_position.z.clamp(-4.0, 3.0);
        self.character_state.look_at_strength =
            self.character_state.look_at_strength.clamp(0.0, 1.0);
        self.graphics.mask_render_downsample =
            normalize_mask_render_downsample(self.graphics.mask_render_downsample);
        self.graphics.target_fps = normalize_target_fps(self.graphics.target_fps);
    }

    pub fn character_settings_path(&self) -> PathBuf {
        ene_config::character_settings_path(&self.current_entry().name)
    }

    fn motion_path_relative(&self) -> String {
        let prefix = format!("characters/{}/", self.current_entry().name);
        self.current_motion()
            .strip_prefix(&prefix)
            .unwrap_or("")
            .to_string()
    }

    pub fn save_per_character_settings(&self) {
        let per = CharacterConfig {
            character_position: [
                self.character_state.character_position.x,
                self.character_state.character_position.y,
                self.character_state.character_position.z,
            ],
            selected_motion_path: self.motion_path_relative(),
            model_scale: self.character_state.model_scale,
            look_at_strength: self.character_state.look_at_strength,
            default_motion: self.motion_path_relative(),
            expressions: None,
        };
        let mut sections = serde_json::Map::new();
        if let Ok(val) = serde_json::to_value(&per) {
            sections.insert("character_settings".to_string(), val);
        }
        if let Ok(json) = serde_json::to_string_pretty(&sections) {
            let path = self.character_settings_path();
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Err(e) = fs::write(path, json) {
                eprintln!("[Config] Failed to save per-character settings: {e}");
            }
        }
    }

    pub fn load_per_character_settings(&mut self) {
        let path = self.character_settings_path();
        let Ok(json) = fs::read_to_string(&path) else {
            return;
        };

        let per = serde_json::from_str::<HashMap<String, serde_json::Value>>(&json)
            .ok()
            .and_then(|map| {
                map.get("character_config")
                    .cloned()
                    .and_then(|v| serde_json::from_value::<CharacterConfig>(v).ok())
            })
            .or_else(|| serde_json::from_str::<CharacterConfig>(&json).ok());

        match per {
            Some(per) => {
                self.character_state.character_position = Vec3::new(
                    per.character_position[0],
                    per.character_position[1],
                    per.character_position[2],
                );
                self.character_state.model_scale = per.model_scale;
                self.character_state.look_at_strength = per.look_at_strength;
                if !per.selected_motion_path.is_empty() {
                    let abs_path = format!(
                        "characters/{}/{}",
                        self.current_entry().name,
                        per.selected_motion_path
                    );
                    if let Some(m) = self
                        .current_entry()
                        .motion_paths
                        .iter()
                        .position(|p| p == &abs_path)
                    {
                        self.character_state.selected_motion = m;
                    }
                }
            }
            None => {
                eprintln!(
                    "[Config] Failed to parse per-character settings {}",
                    path.display()
                );
            }
        }
    }

    pub fn select_character(&mut self, idx: usize) {
        if idx >= self.characters.len() || idx == self.character_state.selected_character {
            return;
        }
        self.save_per_character_settings();
        self.character_state.selected_character = idx;
        self.character_state.selected_motion = 0;
        self.sync_card_path();
        self.load_per_character_settings();
        self.character_state.needs_respawn = true;
    }

    pub fn save(&self) {
        let mut saved = self.ai.ai.clone();
        saved.version = 1;
        let desktop = DesktopSection {
            graphics: self.graphics.clone(),
        };
        if let Err(e) = saved.set_section(&desktop) {
            eprintln!("[Config] Failed to set desktop section: {e}");
        }
        if let Err(e) = ene_config::save_full_config(&saved) {
            eprintln!("[Config] Failed to save config: {e}");
        }
        self.save_per_character_settings();
    }

    pub fn load_from_file(&mut self) {
        let path = ene_config::config_file_path();
        let full = ene_config::load_config_from(&self.assets_dir, &path);

        self.ai.ai = full.clone();

        // Character selection
        let card_path = ene_config::resolve_character_path(&full.character);
        if !card_path.is_empty() {
            let path = Path::new(&card_path);
            if let Some(parent) = path.parent()
                && let Some(name_os) = parent.file_name()
            {
                let name = name_os.to_string_lossy();
                if let Some(idx) = self.characters.iter().position(|c| c.name == name) {
                    self.character_state.selected_character = idx;
                }
            }
        }

        // Graphics settings
        let desktop_cfg = full.get_section::<DesktopSection>().unwrap_or_default();
        self.graphics = desktop_cfg.graphics;

        self.clamp_runtime_values();
        self.load_per_character_settings();
    }
}

pub use ene_config::CharacterConfig;

fn discover_characters(assets_dir: &Path) -> Vec<CharacterEntry> {
    let mut out = Vec::new();
    let characters_dir = assets_dir.join("characters");
    let Ok(dir) = fs::read_dir(&characters_dir) else {
        return out;
    };
    for entry in dir.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let folder = path.file_name().unwrap().to_string_lossy().to_string();
        let card_path = path
            .join("character.json")
            .exists()
            .then(|| path.join("character.json"))
            .or_else(|| {
                path.join("charactor.json")
                    .exists()
                    .then(|| path.join("charactor.json"))
            })
            .unwrap_or_else(|| path.join("character.json"));
        if !card_path.exists() {
            continue;
        }
        let (name, default_motion) =
            read_character_json_meta(&card_path).unwrap_or((folder.clone(), None));

        let mut vrm_paths = Vec::new();
        let mut motion_paths = Vec::new();
        if let Ok(entries) = fs::read_dir(&path) {
            for file in entries.flatten() {
                let file_path = file.path();
                if file_path.is_dir() {
                    continue;
                }
                let relative = format!(
                    "characters/{}/{}",
                    folder,
                    file_path.file_name().unwrap().to_string_lossy()
                );
                if file_path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("vrm"))
                {
                    vrm_paths.push(relative);
                }
            }
        }
        let motions_dir = path.join("motions");
        if let Ok(entries) = fs::read_dir(&motions_dir) {
            for file in entries.flatten() {
                let file_path = file.path();
                if file_path.is_dir() {
                    continue;
                }
                let relative = format!(
                    "characters/{}/motions/{}",
                    folder,
                    file_path.file_name().unwrap().to_string_lossy()
                );
                if file_path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("vrma"))
                {
                    motion_paths.push(relative);
                }
            }
        }
        vrm_paths.sort();
        motion_paths.sort();

        let entry = CharacterEntry {
            name,
            folder: folder.clone(),
            vrm_paths,
            motion_paths,
            card_path: format!("characters/{folder}/character.json"),
            default_motion,
        };
        out.push(entry);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn read_character_json_meta(path: &Path) -> Option<(String, Option<String>)> {
    let content = fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    let name = v.get("data")?.get("name")?.as_str()?.to_string();

    let default_motion = (|| {
        let parent = path.parent()?;
        let folder = parent.file_name()?.to_string_lossy();
        let settings_path = ene_config::character_settings_path(&folder);
        if settings_path.exists() {
            let s = fs::read_to_string(settings_path).ok()?;
            let per: CharacterConfig = serde_json::from_str(&s).ok()?;
            if !per.default_motion.is_empty() {
                return Some(per.default_motion);
            }
        }
        None
    })();

    Some((name, default_motion))
}
