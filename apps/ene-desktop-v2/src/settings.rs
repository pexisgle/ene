//! Plain-data runtime settings (the v2 analog of Bevy
//! `CharacterSettings`).
//!
//! `CharacterSettings` is owned directly by the
//! [`Runtime`](crate::runtime::Runtime) — no global registry, no
//! lock around the outer struct. The only field that is shared
//! across threads (`store`) sits behind a [`parking_lot::RwLock`].
//!
//! Methods mirror the legacy `app_config.rs` shape 1:1 so the
//! eventual port of the settings UI (PR2) is a straight
//! import-and-call exercise.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ene_config::CharacterConfig;
use ene_config::serde::{Deserialize, Serialize};
use glam::Vec3;
use parking_lot::RwLock;

// ── Persisted section (matches `DesktopSection` in legacy) ──

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

#[derive(Debug, Clone, Serialize, Deserialize, ene_config::schemars::JsonSchema)]
#[serde(default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct GraphicsSection {
    pub mask_render_downsample: u32,
    pub target_fps: u32,
    pub shadow_quality: ShadowQuality,
    pub antialiasing_mode: AntialiasingMode,
}

impl Default for GraphicsSection {
    fn default() -> Self {
        Self {
            mask_render_downsample: DEFAULT_MASK_RENDER_DOWNSAMPLE,
            target_fps: DEFAULT_TARGET_FPS,
            shadow_quality: DEFAULT_SHADOW_QUALITY,
            antialiasing_mode: DEFAULT_ANTIALIASING_MODE,
        }
    }
}

ene_config::define_config!(
    settings,
    "desktop",
    pub struct DesktopSection {
        pub graphics: GraphicsSection,
    }
);

pub use GraphicsSection as GraphicsSettings;

// ── Defaults & cycle choices ──

pub const DEFAULT_CHARACTER_NAME: &str = "Alicia";
pub const DEFAULT_VRM_PATH: &str = "characters/Alicia/AliciaSolid.vrm";
pub const DEFAULT_VRMA_PATH: &str = "characters/Alicia/motions/VRMA_01.vrma";
#[allow(dead_code)] // PR3 will use it for the primary window's app-id.
pub const APP_ID: &str = "dev.pexisgle.ene";
#[allow(dead_code)] // PR3 will use it for the primary window's initial size.
pub const WINDOW_WIDTH: u32 = 560;
#[allow(dead_code)] // PR3 will use it for the primary window's initial size.
pub const WINDOW_HEIGHT: u32 = 980;
pub const MASK_RENDER_DOWNSAMPLE_CHOICES: [u32; 3] = [4, 6, 8];
pub const DEFAULT_MASK_RENDER_DOWNSAMPLE: u32 = 8;
pub const TARGET_FPS_CHOICES: [u32; 5] = [15, 30, 60, 120, 0];
pub const DEFAULT_TARGET_FPS: u32 = 60;
#[allow(dead_code)] // PR3 will cycle the shadow quality from the settings UI.
pub const SHADOW_QUALITY_CHOICES: [ShadowQuality; 3] = [
    ShadowQuality::Low,
    ShadowQuality::Medium,
    ShadowQuality::High,
];
pub const DEFAULT_SHADOW_QUALITY: ShadowQuality = ShadowQuality::Medium;
#[allow(dead_code)] // PR3 will cycle the AA mode from the settings UI.
pub const ANTIALIASING_MODE_CHOICES: [AntialiasingMode; 4] = [
    AntialiasingMode::Off,
    AntialiasingMode::Fxaa,
    AntialiasingMode::Smaa,
    AntialiasingMode::Taa,
];
pub const DEFAULT_ANTIALIASING_MODE: AntialiasingMode = AntialiasingMode::Fxaa;

pub fn cycle_mask_render_downsample(current: u32, step: isize) -> u32 {
    cycle_choice(
        &MASK_RENDER_DOWNSAMPLE_CHOICES,
        current,
        step,
        DEFAULT_MASK_RENDER_DOWNSAMPLE,
    )
}

pub fn cycle_target_fps(current: u32, step: isize) -> u32 {
    cycle_choice(&TARGET_FPS_CHOICES, current, step, DEFAULT_TARGET_FPS)
}

#[allow(dead_code)] // PR3 will call this from the Graphics settings page.
pub fn cycle_shadow_quality(current: ShadowQuality, step: isize) -> ShadowQuality {
    cycle_choice(
        &SHADOW_QUALITY_CHOICES,
        current,
        step,
        DEFAULT_SHADOW_QUALITY,
    )
}

#[allow(dead_code)] // PR3 will call this from the Graphics settings page.
pub fn cycle_antialiasing_mode(current: AntialiasingMode, step: isize) -> AntialiasingMode {
    cycle_choice(
        &ANTIALIASING_MODE_CHOICES,
        current,
        step,
        DEFAULT_ANTIALIASING_MODE,
    )
}

fn cycle_choice<T: Copy + PartialEq>(choices: &[T], current: T, step: isize, _default: T) -> T {
    let index = choices.iter().position(|c| *c == current).unwrap_or(1);
    let len = choices.len() as isize;
    let next = (index as isize + step).rem_euclid(len) as usize;
    choices[next]
}

#[allow(dead_code)] // PR3 will call this from the Graphics settings page.
pub fn target_fps_label(target_fps: u32) -> String {
    if target_fps == 0 {
        "Unlimited".to_string()
    } else {
        format!("{target_fps} FPS")
    }
}

// ── CLI parsing ──

/// Read optional VRM and VRMA overrides from the first two CLI
/// arguments. Mirrors the legacy `read_cli_paths` exactly.
pub fn read_cli_paths() -> (String, String) {
    let vrm = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_VRM_PATH.to_string());
    let vrma = std::env::args()
        .nth(2)
        .unwrap_or_else(|| DEFAULT_VRMA_PATH.to_string());
    (vrm, vrma)
}

// ── Discovered character entries ──

#[derive(Debug, Clone)]
pub struct CharacterEntry {
    pub name: String,
    #[allow(dead_code)] // Mirrored from legacy; PR3 will use it for the character folder.
    pub folder: String,
    pub vrm_paths: Vec<String>,
    pub motion_paths: Vec<String>,
    pub motion_names: Vec<String>,
    pub card_path: String,
    pub default_motion: Option<String>,
}

// ── Runtime state shapes (not persisted as JSON) ──

#[derive(Clone, Debug)]
pub struct CharacterState {
    pub selected_character: usize,
    pub selected_motion: usize,
    #[allow(dead_code)] // PR3 will set this when the user picks a new character.
    pub needs_respawn: bool,
    pub model_scale: f32,
    pub character_position: Vec3,
    pub look_at_strength: f32,
    pub motion_override: Option<String>,
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
            motion_override: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct UiState {
    pub settings_window_visible: bool,
    #[allow(dead_code)] // PR5 will toggle the Linux-only Wayland mask debug overlay.
    pub debug_overlay_visible: bool,
    #[allow(dead_code)] // PR2 will bind this to the AI page chat input.
    pub ai_chat_input: String,
    #[allow(dead_code)] // PR2 will display this in the AI page "Latest Response" panel.
    pub ai_latest_response: String,
}

#[derive(Clone, Debug, Default)]
pub struct AiConfig {
    pub ai: ene_config::EneConfig,
}

// ── Top-level settings (the runtime's single source of truth) ──

pub struct CharacterSettings {
    pub assets_dir: PathBuf,
    pub characters: Vec<CharacterEntry>,
    pub graphics: GraphicsSettings,
    pub character_state: CharacterState,
    pub ui: UiState,
    pub ai: AiConfig,
    /// Shared with the AI bridge bootstrap task: that task loads
    /// the on-disk config into a fresh `ConfigStore`, then the
    /// runtime reads it back here via `load_from_file`.
    pub store: Arc<RwLock<ene_config::ConfigStore>>,
}

impl std::fmt::Debug for CharacterSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CharacterSettings")
            .field("assets_dir", &self.assets_dir)
            .field("characters", &self.characters)
            .field("graphics", &self.graphics)
            .field("character_state", &self.character_state)
            .field("ui", &self.ui)
            .field("ai", &self.ai)
            .finish()
    }
}

/// All `CharacterSettings` methods are kept identical in shape to
/// the legacy Bevy `app_config.rs` so PR2 can import-and-call them.
/// PR1 only uses a subset; the unused methods are expected to be
/// wired up by the upcoming PRs (PR2 = settings UI, PR3 = character).
#[allow(dead_code)]
impl CharacterSettings {
    /// Build the initial settings: discover characters on disk,
    /// load the persisted JSON, clamp runtime values.
    pub fn discover(assets_dir: &Path, default_vrm: String) -> Self {
        let mut characters = discover_characters(assets_dir);
        if characters.is_empty() {
            characters.push(CharacterEntry {
                name: DEFAULT_CHARACTER_NAME.to_string(),
                folder: DEFAULT_CHARACTER_NAME.to_string(),
                vrm_paths: vec![DEFAULT_VRM_PATH.to_string()],
                motion_paths: vec![DEFAULT_VRMA_PATH.to_string()],
                motion_names: vec!["VRMA_01".to_string()],
                card_path: format!("characters/{DEFAULT_CHARACTER_NAME}/character.json"),
                default_motion: None,
            });
        }

        let selected_character = characters
            .iter()
            .position(|c| c.vrm_paths.iter().any(|v| v == &default_vrm))
            .unwrap_or(0);

        let default_card = format!("characters/{DEFAULT_CHARACTER_NAME}/character.json");
        let selected_card = characters
            .get(selected_character)
            .map_or(default_card, |c| c.card_path.clone());

        let selected_motion = characters
            .get(selected_character)
            .and_then(|entry| {
                entry
                    .default_motion
                    .as_ref()
                    .and_then(|dm| entry.motion_names.iter().position(|n| n == dm))
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
                    character: format!("{}/{}", assets_dir.display(), selected_card),
                    ..Default::default()
                },
            },
            store: Arc::new(RwLock::new(ene_config::ConfigStore::from_config(
                ene_config::EneConfig::default(),
            ))),
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

    #[expect(dead_code)]
    pub fn current_motion(&self) -> &str {
        if let Some(ref override_path) = self.character_state.motion_override {
            return override_path;
        }
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
            cycle_mask_render_downsample(self.graphics.mask_render_downsample, 0);
        self.graphics.target_fps = cycle_target_fps(self.graphics.target_fps, 0);
    }

    pub fn save_per_character_settings(&self) {
        self.sync_to_store();
        let char_name = self.current_entry().name.clone();
        let store = self.store.read();
        if let Err(e) = store.flush(Some(&char_name)) {
            tracing::warn!("[Config] Failed to save per-character settings: {e}");
        }
    }

    pub fn load_per_character_settings(&mut self) {
        let char_name = self.current_entry().name.clone();
        let store = self.store.read();
        store.load_character_config(&char_name);
        let per = store.character_config();

        self.character_state.character_position = Vec3::new(
            per.character_position[0],
            per.character_position[1],
            per.character_position[2],
        );
        self.character_state.model_scale = per.model_scale;
        self.character_state.look_at_strength = per.look_at_strength;
        if !per.default_motion.is_empty()
            && let Some(m) = self
                .current_entry()
                .motion_names
                .iter()
                .position(|n| n == &per.default_motion)
        {
            self.character_state.selected_motion = m;
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
        self.sync_to_store();
        let char_name = self.current_entry().name.clone();
        let store = self.store.read();
        if let Err(e) = store.flush(Some(&char_name)) {
            tracing::warn!("[Config] Failed to save config: {e}");
        }
    }

    pub fn mark_dirty(&self) {
        self.sync_to_store();
    }

    /// Called once per frame by the runtime; flushes the
    /// underlying `ConfigStore` to disk only if anything was
    /// changed since the last flush.
    pub fn flush_if_dirty(&self) {
        let char_name = self.current_entry().name.clone();
        let store = self.store.read();
        let _ = store.flush_if_dirty(Some(&char_name));
    }

    fn sync_to_store(&self) {
        let mut config = self.ai.ai.clone();
        config.version = 1;
        let desktop = DesktopSection {
            graphics: self.graphics.clone(),
        };
        if let Err(e) = config.set_section(&desktop) {
            tracing::warn!("[Config] Failed to set desktop section: {e}");
        }
        let store = self.store.read();
        store.set_config(config);

        let entry = self.current_entry();
        let default_motion_name = entry
            .motion_names
            .get(self.character_state.selected_motion)
            .cloned()
            .unwrap_or_default();
        let existing_extra = store.character_config().extra;
        let char_config = CharacterConfig {
            character_position: [
                self.character_state.character_position.x,
                self.character_state.character_position.y,
                self.character_state.character_position.z,
            ],
            model_scale: self.character_state.model_scale,
            look_at_strength: self.character_state.look_at_strength,
            default_motion: default_motion_name,
            default_expression: "neutral".to_string(),
            extra: existing_extra,
        };
        store.set_character_config(char_config);
    }

    pub fn load_from_file(&mut self) {
        let path = ene_config::config_file_path();
        let full = ene_config::load_config_from(&self.assets_dir, &path);

        self.ai.ai = full.clone();
        *self.store.write() = ene_config::ConfigStore::from_config(full.clone());

        // Character selection: derive from `full.character` (a
        // card name or full path).
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

        self.clamp_runtime_values();
        self.load_per_character_settings();
    }
}

// ── File-system discovery (private) ──

fn discover_characters(assets_dir: &Path) -> Vec<CharacterEntry> {
    let mut out = Vec::new();
    let characters_dir = assets_dir.join("characters");
    let Ok(dir) = std::fs::read_dir(&characters_dir) else {
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
        let (name, default_motion_name, card_motions) =
            read_character_json_meta(&card_path).unwrap_or((folder.clone(), None, None));

        let mut vrm_paths = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&path) {
            for file in entries.flatten() {
                let file_path = file.path();
                if file_path.is_dir() {
                    continue;
                }
                let relative = format!(
                    "characters/{folder}/{}",
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
        vrm_paths.sort();

        let mut motion_paths = Vec::new();
        if let Some(motions) = card_motions {
            for m in motions {
                motion_paths.push(format!("characters/{folder}/{}", m.path));
            }
        } else {
            let motions_dir = path.join("motions");
            if let Ok(entries) = std::fs::read_dir(&motions_dir) {
                for file in entries.flatten() {
                    let file_path = file.path();
                    if file_path.is_dir() {
                        continue;
                    }
                    let relative = format!(
                        "characters/{folder}/motions/{}",
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
        }
        motion_paths.sort();

        let motion_names = motion_paths
            .iter()
            .map(|p| {
                std::path::Path::new(p)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();

        out.push(CharacterEntry {
            name,
            folder: folder.clone(),
            vrm_paths,
            motion_names,
            motion_paths,
            card_path: format!("characters/{folder}/character.json"),
            default_motion: default_motion_name,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn read_character_json_meta(
    path: &Path,
) -> Option<(String, Option<String>, Option<Vec<ene_config::MotionEntry>>)> {
    let content = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    let name = v.get("data")?.get("name")?.as_str()?.to_string();

    let default_motion = (|| {
        let parent = path.parent()?;
        let folder = parent.file_name()?.to_string_lossy();
        let settings_path = ene_config::character_settings_path(&folder);
        if settings_path.exists() {
            let s = std::fs::read_to_string(settings_path).ok()?;
            let per: CharacterConfig = serde_json::from_str(&s).ok()?;
            if !per.default_motion.is_empty() {
                return Some(per.default_motion);
            }
        }
        None
    })();

    let motions = (|| {
        let extensions = v.get("data")?.get("extensions")?;
        let ene = extensions.get("ene")?;
        let motions_val = ene.get("motions")?;
        let motions: Vec<ene_config::MotionEntry> =
            serde_json::from_value(motions_val.clone()).ok()?;
        Some(motions)
    })();

    Some((name, default_motion, motions))
}
