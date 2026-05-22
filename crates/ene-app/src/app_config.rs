use bevy::prelude::*;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use bevy::window::CompositeAlphaMode;
#[cfg(target_os = "windows")]
use bevy::window::PresentMode;
use bevy::window::{WindowLevel, WindowMode, WindowPlugin, WindowResolution};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_CHARACTER_NAME: &str = "Alicia";
pub const DEFAULT_VRM_PATH: &str = "characters/Alicia/AliciaSolid.vrm";
pub const DEFAULT_VRMA_PATH: &str = "characters/Alicia/motions/VRMA_01.vrma";
pub const APP_ID: &str = "dev.pexisgle.Ene";
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShadowQuality {
    Low,
    #[default]
    Medium,
    High,
}

impl ShadowQuality {
    pub fn label(self) -> &'static str {
        match self {
            ShadowQuality::Low => "Low",
            ShadowQuality::Medium => "Medium",
            ShadowQuality::High => "High",
        }
    }

    pub fn shadow_map_size(self) -> usize {
        match self {
            ShadowQuality::Low => 1_024,
            ShadowQuality::Medium => 2_048,
            ShadowQuality::High => 4_096,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AntialiasingMode {
    Off,
    #[default]
    Fxaa,
    Smaa,
    Taa,
}

impl AntialiasingMode {
    pub fn label(self) -> &'static str {
        match self {
            AntialiasingMode::Off => "Off",
            AntialiasingMode::Fxaa => "FXAA",
            AntialiasingMode::Smaa => "SMAA",
            AntialiasingMode::Taa => "TAA",
        }
    }
}

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
        format!("{} FPS", target_fps)
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
            title: "Ene".to_string(),
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
    #[allow(dead_code)]
    pub folder: String,
    pub vrm_paths: Vec<String>,
    pub motion_paths: Vec<String>,
    pub card_path: String,
    pub default_motion: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphicsSettings {
    pub mask_render_downsample: u32,
    pub target_fps: u32,
    pub shadow_quality: ShadowQuality,
    pub antialiasing_mode: AntialiasingMode,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Self {
            mask_render_downsample: DEFAULT_MASK_RENDER_DOWNSAMPLE,
            target_fps: DEFAULT_TARGET_FPS,
            shadow_quality: DEFAULT_SHADOW_QUALITY,
            antialiasing_mode: DEFAULT_ANTIALIASING_MODE,
        }
    }
}

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

#[derive(Clone, Debug, Default)]
pub struct UiState {
    pub settings_window_visible: bool,
    pub debug_overlay_visible: bool,
    pub ai_chat_input: String,
    pub ai_latest_response: String,
}

#[derive(Clone, Debug, Default)]
pub struct AiConfig {
    pub ai: ene_ai_core::config::AiSettings,
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
                card_path: format!("characters/{}/character.json", DEFAULT_CHARACTER_NAME),
                default_motion: None,
            });
        }

        let selected_character = characters
            .iter()
            .position(|c| c.vrm_paths.iter().any(|v| v == &default_vrm))
            .unwrap_or(0);

        let default_card_path = format!("characters/{}/character.json", DEFAULT_CHARACTER_NAME);
        let selected_card_path = characters
            .get(selected_character)
            .map(|c| c.card_path.clone())
            .unwrap_or(default_card_path);
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
                ai: ene_ai_core::config::AiSettings {
                    character_card_path: format!("{}/{}", assets_dir.display(), selected_card_path),
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
        self.ai.ai.character_card_path = path;
    }

    pub fn clamp_runtime_values(&mut self) {
        self.character_state.model_scale = self.character_state.model_scale.clamp(0.25, 4.0);
        self.character_state.character_position.x = self.character_state.character_position.x.clamp(-3.0, 3.0);
        self.character_state.character_position.y = self.character_state.character_position.y.clamp(-2.0, 3.0);
        self.character_state.character_position.z = self.character_state.character_position.z.clamp(-4.0, 3.0);
        self.character_state.look_at_strength = self.character_state.look_at_strength.clamp(0.0, 1.0);
        self.graphics.mask_render_downsample = normalize_mask_render_downsample(self.graphics.mask_render_downsample);
        self.graphics.target_fps = normalize_target_fps(self.graphics.target_fps);
    }

    pub fn character_settings_path(&self) -> PathBuf {
        self.assets_dir
            .join("characters")
            .join(&self.current_entry().name)
            .join("character_settings.json")
    }

    fn motion_path_relative(&self) -> String {
        let prefix = format!("characters/{}/", self.current_entry().name);
        self.current_motion()
            .strip_prefix(&prefix)
            .unwrap_or("")
            .to_string()
    }

    pub fn save_per_character_settings(&self) {
        let per = CharacterPerSettings {
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
        if let Ok(json) = serde_json::to_string_pretty(&per) {
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
        match serde_json::from_str::<CharacterPerSettings>(&json) {
            Ok(per) => {
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
            Err(e) => {
                eprintln!(
                    "[Config] Failed to parse per-character settings {}: {e}",
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
        let saved = AppSettings::from_character_settings(self);
        match serde_json::to_string_pretty(&saved) {
            Ok(json) => {
                let path = ene_ai_core::paths::config_file_path();
                if let Err(e) = fs::write(path, json) {
                    eprintln!("[Config] Failed to save settings: {e}");
                }
            }
            Err(e) => {
                eprintln!("[Config] Failed to serialize settings: {e}");
            }
        }
        self.save_per_character_settings();
    }

    pub fn load_from_file(&mut self) {
        let path = ene_ai_core::paths::config_file_path();
        let Ok(json) = fs::read_to_string(&path) else {
            return;
        };

        // ai-core に委譲: AiSettings の読み取り + character_card_path 解決
        self.ai.ai = ene_ai_core::config::load_settings_from(&self.assets_dir, &path);

        // アプリ固有フィールドは raw JSON から別途パース
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
            // キャラクター選択
            if let Some(name) = value.get("character").and_then(|c| c.as_str()) {
                if !name.is_empty() {
                    if let Some(idx) = self.characters.iter().position(|c| c.name == name) {
                        self.character_state.selected_character = idx;
                    }
                }
            }
            // グラフィック設定
            if let Some(app_val) = value.get("app") {
                if let Ok(app) = serde_json::from_value::<AppSection>(app_val.clone()) {
                    self.graphics.mask_render_downsample = app.graphics.mask_render_downsample;
                    self.graphics.target_fps = app.graphics.target_fps;
                    self.graphics.shadow_quality = app.graphics.shadow_quality;
                    self.graphics.antialiasing_mode = app.graphics.antialiasing_mode;
                }
            }
        }

        self.clamp_runtime_values();
        self.load_per_character_settings();
    }
}

const CONFIG_VERSION: u32 = 1;

/// Per-character settings stored in `characters/{name}/character_settings.json`
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
struct CharacterPerSettings {
    character_position: [f32; 3],
    selected_motion_path: String,
    model_scale: f32,
    look_at_strength: f32,
    default_motion: String,
    expressions: Option<serde_json::Value>,
}

impl Default for CharacterPerSettings {
    fn default() -> Self {
        Self {
            character_position: [0.0, 0.0, 0.0],
            selected_motion_path: String::new(),
            model_scale: 1.0,
            look_at_strength: 0.6,
            default_motion: String::new(),
            expressions: None,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct AppSettings {
    version: u32,
    character: String,
    app: AppSection,
    #[serde(flatten)]
    ai: ene_ai_core::config::AiSettings,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct AppSection {
    graphics: GraphicsSection,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct GraphicsSection {
    mask_render_downsample: u32,
    target_fps: u32,
    shadow_quality: ShadowQuality,
    antialiasing_mode: AntialiasingMode,
}

impl AppSettings {
    fn from_character_settings(s: &CharacterSettings) -> Self {
        AppSettings {
            version: CONFIG_VERSION,
            character: s.current_entry().name.clone(),
            app: AppSection {
                graphics: GraphicsSection {
                    mask_render_downsample: s.graphics.mask_render_downsample,
                    target_fps: s.graphics.target_fps,
                    shadow_quality: s.graphics.shadow_quality,
                    antialiasing_mode: s.graphics.antialiasing_mode,
                },
            },
            ai: s.ai.ai.clone(),
        }
    }

}

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
            card_path: format!("characters/{}/character.json", folder),
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

    let default_motion = path
        .parent()
        .and_then(|p| p.join("character_settings.json").exists().then(|| p.join("character_settings.json")))
        .and_then(|settings_path| {
            let s = fs::read_to_string(settings_path).ok()?;
            let sv: serde_json::Value = serde_json::from_str(&s).ok()?;
            sv.get("default_motion")
                .and_then(|m| m.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        });

    Some((name, default_motion))
}
