# `ene-config` — API Reference

> **Crate:** `ene-config`  
> **Role:** Configuration loading, schema generation, and character card types for the Ene system.

---

## Overview

`ene-config` manages all runtime configuration using the [`figment`](https://docs.rs/figment) layered configuration system. It defines the `EneConfig` type, the `define_config!` macro for declaring typed sections, global config state, and the character card format. It also owns `Truncate` / `TruncateResult` (`ene_config::truncate`), folded from the former `ene-common` crate; tools typically use the re-export in [`ene-tool-common`](./ene-tool-common.md).

### Loading Order

Configuration is resolved in priority order (later layers win):

```
1. Compile-time defaults  (hardcoded in define_config! blocks)
         ↓
2. assets/settings.json   (user's settings file)
         ↓
3. ENE_* environment vars (e.g. ENE_LLM__API_KEY)
```

> **Note:** `assets/settings.schema.json` and `character_settings.schema.json` are auto-generated and gitignored. Do not commit or hand-edit them — they are regenerated on every `cargo run -p ene-cli`.

---

## `EneConfig`

The top-level configuration container. Internally it holds a map of section keys to typed section data.

```rust
pub struct EneConfig { /* opaque */ }
```

The full type (defined in `crates/ene-config/src/config.rs`):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EneConfig {
    /// Schema version number.
    pub version: u32,
    /// Character card name or path.
    pub character: String,
    /// Display name shown to the user.
    pub user_name: String,
    /// Behavioural rules injected into every system prompt.
    pub runtime_rules: String,

    #[serde(flatten)]
    #[schemars(skip)]
    /// Catch-all for provider, tool, and other sub-configurations.
    pub extra: BTreeMap<String, serde_json::Value>,
}
```

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `get_section` | `fn get_section<T>(&self) -> Result<T, EneConfigError> where T: DeserializeOwned + Default + HasConfigKey` | Deserialise a sub-section by `T::KEY`. Returns `Ok(T::default())` when the key is absent. |
| `set_section` | `fn set_section<T>(&mut self, section: &T) -> Result<(), EneConfigError> where T: Serialize + HasConfigKey` | Serialise and insert a sub-section under `T::KEY`. |

---

## `define_config!` Macro

The `define_config!` macro declares a typed configuration section. Each section is independently serializable, deserializable, and has a unique registry key. There are three variants:

1. `settings, "key", …` — registers the section under `settings.json`.
2. `character, "key", …` — registers the section under `character_settings.json`.
3. `$parent, "key", …` — nests the section under another section, inheriting its `ConfigTarget`.

```rust
ene_config::define_config! {
    settings,
    "llm",
    /// LLM backend configuration.
    pub struct LlmConfig {
        /// Base URL for the OpenAI-compatible API.
        pub api_base: String = "https://api.openai.com/v1".to_string(),

        /// API key (also settable via ENE_LLM__API_KEY).
        pub api_key: String = String::new(),

        /// Model name to use for chat completions.
        pub model: String = "gpt-4o".to_string(),

        /// Maximum context window in tokens.
        pub max_tokens: usize = 4096,
    }
}
```

The macro:
1. Derives `Debug`, `Clone`, `serde::Serialize`, `serde::Deserialize`,
   and `schemars::JsonSchema`.
2. Implements `HasConfigKey` for the type (with `const KEY` set to the
   supplied string and `path()` returning the path from the root).
3. Generates a `Default` impl using each field's inline `= default` (or
   `Default::default()` when omitted).
4. Registers the JSON Schema with `__register_schema` (via a `ctor` hook).

A companion macro `define_tool_config!` is provided for tool config
schemas; it uses `__register_tool_schema` instead.

---

## Global Config Functions

### `load_config`

```rust
pub fn load_config() -> Result<EneConfig, EneConfigError>
```

Loads configuration using the default paths (`assets/` directory next to the binary). This is the primary entry point for application startup. Internally calls `load_full_config()`. Returns `Err(EneConfigError::GenericConfigError(..))` if `settings.json` is malformed or an `ENE_*` env var cannot be parsed into the expected type — it does **not** silently fall back to defaults on a bad config file. (See [`ConfigStore::load`](#configstore) for a call site that intentionally still falls back, for host startup ergonomics.)

### `load_config_from`

```rust
pub fn load_config_from(assets_dir: &Path, config_path: &Path) -> Result<EneConfig, EneConfigError>
```

Loads configuration from an explicit directory and file path. Useful for tests or non-standard deployments. Internally calls `load_full_config_from`.

### `load_full_config` / `load_full_config_from`

```rust
pub fn load_full_config() -> Result<EneConfig, EneConfigError>
pub fn load_full_config_from(assets_dir: &Path, config_path: &Path) -> Result<EneConfig, EneConfigError>
```

Full `EneConfig` load. Layers, in priority order: `EneConfig::default()` →
`settings.json` → `ENE_*` environment variables (`__`-delimited, case-folded
to lowercase so `ENE_PROVIDER__API_KEY` maps to the `provider.api_key`
path). On success, writes generated schemas to the assets dir and updates
the global singleton via `update_global_config` before returning `Ok`.
On failure (malformed JSON, a type-mismatched env var, etc.) returns
`Err(EneConfigError::GenericConfigError(..))` **without** touching the
global singleton or the schema files on disk.

### `save_full_config`

```rust
pub fn save_full_config(config: &EneConfig) -> Result<(), std::io::Error>
```

Serialise `config` to JSON, write it to the standard settings file, and
update the global singleton.

### `update_section`

```rust
pub fn update_section<T>(value: &T) -> Result<(), EneConfigError>
where T: Serialize + DeserializeOwned + HasConfigKey
```

Loads the current config, applies `config.set_section(value)`, and saves
in one call.

### `register_runtime_schema`

```rust
pub fn register_runtime_schema(key: &str, schema: serde_json::Value)
```

Registers a JSON Schema fragment under `key` at runtime. Called by
`ToolHostManager` once each tool binary reports its config schema. Not
normally used by hand.

### `write_schemas`

```rust
pub fn write_schemas(assets_dir: &Path)
```

Writes the collected JSON Schema fragments to `settings.schema.json`,
`character_settings.schema.json`, and `character_card.schema.json` in
`assets_dir`. Called during CLI startup to keep schemas in sync.

### `generate_schema_json` / `generate_character_schema_json` / `generate_character_card_schema_json`

```rust
pub fn generate_schema_json() -> Result<String, serde_json::Error>
pub fn generate_character_schema_json() -> Result<String, serde_json::Error>
pub fn generate_character_card_schema_json() -> Result<String, serde_json::Error>
```

Returns the JSON Schema for each respective config file as a pretty
JSON string.

### `resolve_character_path`

```rust
pub fn resolve_character_path(name: &str) -> String
```

Resolves a character name to a filesystem path. Bare names are mapped to
`assets/characters/<name>/character.json`; paths containing `/` or `\` are
returned as-is.

---

### `ensure_resource_dirs`

```rust
pub fn ensure_resource_dirs() -> Result<PathBuf, EneConfigError>
```

Initialises the application data directory. On the first launch, it copies the default assets from the distribution directory into the OS-standard application data directory (e.g., `%APPDATA%` on Windows).

**Distribution Bundle Policy:**
To keep the binary release package lightweight, do not bundle heavy local assets or temporary development files in the distributed `assets/` directory. The following resources listed in `.gitignore` must be excluded from the release archive:
* `assets/models/` (Local model caches)
* Generated schemas under `assets/schema/` (`settings.schema.json`, etc.)
* Database files (`*.db*`) and dotfiles.

---

## Global Config Singleton

```rust
pub static GLOBAL_CONFIG: std::sync::OnceLock<std::sync::RwLock<EneConfig>>;
```

A process-global `RwLock<EneConfig>` initialised on first use. The actor
reads this on startup; use `EneHandle::reconfigure` to update it at
runtime. Convenience accessors are also provided:

| Function | Description |
|----------|-------------|
| `get_global_config() -> EneConfig` | Clone of the current global config (or `EneConfig::default()` if unset). |
| `update_global_config(config: EneConfig)` | Replace the global config in place. |
| `get_global_section<T>() -> T` | Deserialise a section from the global config (returns `T::default()` if missing). |

The `ConfigStore` type (see `store.rs`) is a higher-level wrapper that
adds dirty tracking and auto-save; see [`ConfigStore`](#configstore) below.

---

## `ConfigStore`

`ConfigStore` is the single persistence layer used by `ene-desktop` and
`ene-cli` at runtime. It wraps both the global [`EneConfig`](#eneconfig)
and the active [`CharacterConfig`](#character-config), tracking whether
each has unsaved changes via atomic dirty flags so a periodic flush
system (e.g. a Bevy system that runs once per frame) can call
[`flush_if_dirty`](#configstore-methods) cheaply on every tick without
re-serialising and rewriting files that haven't changed.

```rust
pub struct ConfigStore { /* opaque: RwLock<EneConfig> + RwLock<CharacterConfig> + 2 AtomicBool */ }
```

### Constructors

| Constructor | Signature | Description |
|---|---|---|
| `load` | `fn load() -> Self` | Loads the global config from disk via the standard figment pipeline. On any load failure, logs the error and falls back to `EneConfig::default()` — this is the *only* call site in the crate that preserves the old silent-default behaviour, because a host process must be able to construct a store even before the user has fixed a broken `settings.json`. Character config starts as `CharacterConfig::default()`; call `load_character_config` afterward to populate it. |
| `try_load` | `fn try_load() -> Result<Self, EneConfigError>` | Same as `load`, but propagates the load error instead of silently falling back. Prefer this at CLI startup, where failing fast surfaces config problems to the user immediately. |
| `from_config` | `fn from_config(config: EneConfig) -> Self` | Builds a store from an already-loaded `EneConfig` (e.g. one constructed in a test, or loaded by other means). |

### Methods {#configstore-methods}

| Method | Signature | Description |
|---|---|---|
| `config` | `fn config(&self) -> EneConfig` | Returns a clone of the current global config. |
| `with_config_mut` | `fn with_config_mut(&self, f: impl FnOnce(&mut EneConfig))` | Runs `f` against the global config under a write lock, then marks the store dirty. Preferred over `set_config` for incremental edits. |
| `set_config` | `fn set_config(&self, config: EneConfig)` | Replaces the entire global config and marks the store dirty. |
| `get_section<T>` | `fn get_section<T>(&self) -> T where T: DeserializeOwned + Default + HasConfigKey` | Reads a typed section from the global config; returns `T::default()` on any error (missing key or deserialisation failure), matching `get_global_section`. |
| `set_section<T>` | `fn set_section<T>(&self, section: &T) where T: Serialize + HasConfigKey` | Writes a typed section into the global config and marks the store dirty. Errors from the underlying `EneConfig::set_section` are swallowed (`.ok()`) — this method cannot fail from the caller's perspective. |
| `character_config` | `fn character_config(&self) -> CharacterConfig` | Returns a clone of the current per-character config. |
| `with_character_config_mut` | `fn with_character_config_mut(&self, f: impl FnOnce(&mut CharacterConfig))` | Runs `f` against the per-character config under a write lock, then marks it dirty. |
| `load_character_config` | `fn load_character_config(&self, character_name: &str)` | Reads `character_settings.json` for `character_name` (via `character_settings_path`) and replaces the in-memory `CharacterConfig`. Falls back to `CharacterConfig::default()` if the file is missing or fails to parse. Does **not** mark the store dirty (it's a load, not a mutation). |
| `set_character_config` | `fn set_character_config(&self, config: CharacterConfig)` | Replaces the per-character config and marks it dirty. |
| `get_character_section<T>` | `fn get_character_section<T>(&self) -> T where T: DeserializeOwned + Default + HasConfigKey` | Reads a typed section from the per-character config's `extra` map. |
| `set_character_section<T>` | `fn set_character_section<T>(&self, section: &T) where T: Serialize + HasConfigKey` | Writes a typed section into the per-character config's `extra` map and marks it dirty. |
| `flush_if_dirty` | `fn flush_if_dirty(&self, character_name: Option<&str>) -> std::io::Result<bool>` | Saves the global config to disk if its dirty flag is set (clearing the flag), and the per-character config if *its* dirty flag is set *and* `character_name` is `Some`. Returns `Ok(true)` if anything was written, `Ok(false)` if nothing was dirty. This is the method a per-frame autosave system should call. |
| `flush` | `fn flush(&self, character_name: Option<&str>) -> std::io::Result<()>` | Forces both dirty flags to `true` and calls `flush_if_dirty`, unconditionally writing both configs to disk. Use on shutdown or explicit "Save" actions. |
| `is_dirty` | `fn is_dirty(&self) -> bool` | `true` if either the global or per-character config has unsaved changes. |

---

## Re-exports and Helpers

`ene_config::lib.rs` re-exports the following items from the workspace
crates so downstream code can use a single `ene_config::*` namespace:

| Item | Source |
|------|--------|
| `CharacterCardV3`, `CharacterCardData`, `CharacterAsset`, `EneExtension`, `ExpressionDefinition`, `Lorebook`, `LorebookEntry`, `ResolvedExpression`, `expand_cbs_macros`, `resolve_expressions` | `character_card` |
| `CharacterConfig`, `MotionEntry` | `character_config` |
| `EneConfig`, `HasConfigKey`, `ConfigTarget`, `__register_schema`, `__register_tool_schema`, `__register_tool_schema`, `get_global_config`, `update_global_config`, `get_global_section`, `load_config`, `load_config_from`, `load_full_config`, `load_full_config_from`, `save_full_config`, `update_section`, `register_runtime_schema`, `resolve_character_path`, `generate_schema_json`, `generate_character_schema_json`, `generate_character_card_schema_json`, `write_schemas` | `config` |
| `ConfigError`, `EneConfigError` | `error` |
| `IS_DEV_BUILD`, `app_data_dir`, `assets_dir`, `builtin_tools_dir`, `config_file_path`, `models_dir`, `schema_file_path`, `character_schema_file_path`, `character_card_schema_file_path`, `character_settings_path`, `tool_socket_dir`, `user_tools_dir` | `paths` |
| `ensure_resource_dirs` | `resources` |
| `ConfigStore` | `store` |
| `define_config!`, `define_tool_config!`, `define_label_enum!` | top-level macros |
| `serde`, `schemars`, `ctor` | re-exports |

`EneConfigError` is the canonical error enum; `ConfigError` is a type alias
(`pub type ConfigError = EneConfigError;`).

### `EneConfigError`

```rust
pub enum EneConfigError {
    MissingBaseUrl { env_var: String },
    MissingApiKey { env_var: String },
    NoCharacterCard,
    CardReadError(#[from] std::io::Error),
    JsonError(#[from] serde_json::Error),
    GenericConfigError(String),
    IoError(#[source] std::io::Error),
}
```

| Variant | Meaning |
|---|---|
| `MissingBaseUrl { env_var }` | An AI provider base URL is empty and no fallback env var was set. `env_var` names the variable that was checked (e.g. `"OPENAI_BASE_URL"`). |
| `MissingApiKey { env_var }` | An API key is empty and no fallback env var was set. |
| `NoCharacterCard` | A character card was requested before one was loaded. |
| `CardReadError(std::io::Error)` | I/O failure while reading a character card file from disk. Implements `#[from] std::io::Error`, so `?` on a card read converts automatically. |
| `JsonError(serde_json::Error)` | A character card or config file failed to parse as JSON. Implements `#[from] serde_json::Error`. |
| `GenericConfigError(String)` | Catch-all for configuration errors with a free-form message — this is what `load_config`/`load_full_config_from` return on a bad `settings.json` or a malformed `ENE_*` env var, and what `set_section`/`get_section` return on a serialisation/deserialisation failure or an attempt to read/write a section through the wrong config target (see [`EneConfig::get_section`](#methods)). |
| `IoError(std::io::Error)` | General I/O error that is *not* specifically a character-card read (unlike `CardReadError`, this variant does not implement `#[from]`, so call sites must wrap it explicitly). |

---

## Character Card Types

Ene uses the **CharacterCard V3** format for character data.

### `CharacterCardV3`

```rust
pub struct CharacterCardV3 {
    /// Core character data (name, description, personality, scenario, etc.)
    pub data: CharacterCardData,

    /// Embedded binary assets referenced by the card (images, audio, etc.)
    pub assets: Vec<CharacterAsset>,
}
```

### `CharacterCardData`

Holds all text fields of a character card: `name`, `description`, `personality`, `scenario`, `first_mes`, `alternate_greetings`, `system_prompt`, and extension data.

### `CharacterAsset`

```rust
pub struct CharacterAsset {
    /// Asset MIME type (e.g., `"image/png"`).
    pub asset_type: String,

    /// Logical name used to reference the asset in card text.
    pub name: String,

    /// URI or embedded base64 data.
    pub uri: String,
}
```

### `EneExtension` / `Lorebook` / `LorebookEntry`

The `EneExtension` struct is an open bag of character-card extension
data (e.g. `talkativeness`, `favoured_decision_style`, lorebooks). The
included `Lorebook` and `LorebookEntry` types are the typed shape used by
the desktop UI and the memory system.

### `ExpressionDefinition`

Defines a named facial expression or state associated with a character, used by the desktop renderer.

```rust
pub struct ExpressionDefinition {
    pub name: String,
    pub asset_name: String,
    pub trigger_tokens: Vec<String>,
}
```

### `ResolvedExpression`

The result of resolving an `ExpressionDefinition` against loaded assets — includes the actual image data ready for display.

```rust
pub struct ResolvedExpression {
    pub name: String,
    pub image_data: Vec<u8>,
    pub trigger_tokens: Vec<String>,
}
```

---

## Character Card Functions

### `expand_cbs_macros`

```rust
pub fn expand_cbs_macros(card: &CharacterCardV3) -> String
```

Expands CBS (Character Book Substitution) macro syntax within a card's system prompt. Returns the expanded string ready to inject into the LLM context.

### `resolve_expressions`

```rust
pub fn resolve_expressions(card: &CharacterCardV3) -> Vec<ResolvedExpression>
```

Iterates the card's `assets` and `ExpressionDefinition`s, loading image data and producing a `Vec<ResolvedExpression>` for the desktop UI.

---

## Character Config {#character-config}

`ene_config::CharacterConfig` is the per-character companion to
`EneConfig`. It is loaded from `character_settings.json` next to the
character card and holds UI/runtime preferences for the desktop 3D
renderer — model transform, look-at behaviour, and the default
motion/expression to play on load. Like `EneConfig`, it has a
`#[serde(flatten)]` catch-all `extra` map for typed sections registered
via `define_config!(character, "key", …)`.

```rust
pub struct CharacterConfig {
    pub character_position: [f32; 3],  // default: [0.0, 0.0, 0.0]
    pub model_scale: f32,               // default: 1.0
    pub look_at_strength: f32,           // default: 0.6
    pub default_motion: String,          // default: ""
    pub default_expression: String,      // default: "neutral"
    pub extra: BTreeMap<String, serde_json::Value>,
}
```

| Field | Description |
|---|---|
| `character_position` | 3D position `[x, y, z]` of the character model in the scene. |
| `model_scale` | Uniform scale factor applied to the character model. |
| `look_at_strength` | How strongly the character's gaze tracks the user, from `0.0` (never looks at the user) to `1.0` (always looks at the user). |
| `default_motion` | Name of the motion to play by default; should match a `MotionEntry.name` from the character's motion list. |
| `default_expression` | Name of the expression to apply by default (e.g. `"neutral"`). |
| `extra` | Catch-all map for `character`-target `define_config!` sections. |

### Methods

| Method | Signature | Description |
|---|---|---|
| `get_section<T>` | `fn get_section<T>(&self) -> Result<T, EneConfigError> where T: DeserializeOwned + Default + HasConfigKey` | Deserialise a `character`-target sub-section from `extra` by `T::path()`. Returns `Ok(T::default())` if the path is absent. Debug builds assert `T::TARGET == ConfigTarget::Character`. |
| `set_section<T>` | `fn set_section<T>(&mut self, section: &T) -> Result<(), EneConfigError> where T: Serialize + HasConfigKey` | Serialise and insert a `character`-target sub-section into `extra` by `T::path()`. |

### `MotionEntry`

A single named motion file reference, used in character-specific motion
lists (typically stored as a `Vec<MotionEntry>` inside a custom
`character`-target config section).

```rust
pub struct MotionEntry {
    pub name: String,  // e.g. "VRMA_01"
    pub path: String,  // e.g. "motions/VRMA_01.vrma"
}
```

---

## Prompt Templates: `PromptLibrary`

`PromptLibrary` loads the LLM-facing prompt strings (system prompt
framing, emotion rules, memory/summarizer/extractor/affect-classifier/
proactive-speech templates) used throughout `ene-runtime`, keeping user-facing prose out of
compiled code and giving each string a stable, localisable home.

Structured prompts (decision, affect classifier, extractor, summarizer) follow a shared layout: **Role → Task → Output contract → Field specs → Decision rules → Examples → Constraints**. JSON outputs use **reason-first** field order where applicable (`reason` before decision fields). Plain-text prompts (screen summary, generation hints) specify line limits (typically 2–3 lines). User-turn templates wrap input in labelled sections (`## Conversation`, etc.) to separate data from instructions.

```rust
pub struct PromptLibrary { /* opaque: PromptLibraryData + lang */ }
```

### Constructors

| Constructor | Signature | Description |
|---|---|---|
| `load` | `fn load(lang: &str) -> Self` | Loads the built-in prompt set for a language code. `"ja"`/`"jp"` load the Japanese defaults; anything else (including unrecognised codes) falls back to English. |
| `built_in_english` | `fn built_in_english() -> Self` | Returns the compile-time-embedded English prompts (from `crates/ene-config/prompts/en.json` and the accompanying `.md` fragments via `include_str!`). A parse failure here is a build-time bug, not a runtime condition. |
| `built_in_japanese` | `fn built_in_japanese() -> Self` | Same as above, for Japanese (`prompts/ja.json`). |

### Accessors

| Method | Returns | Description |
|---|---|---|
| `lang()` | `&str` | The language code this library was loaded for (`"en"` or `"ja"`). |
| `system()` | `&SystemPrompts` | System-prompt framing: `mascot_context`, section headers (`behavior_rules_header`, `character_header`, `personality_header`, `background_header`, `scene_header`, `examples_header`). Has `render_mascot_context(char_name, user_name)`. |
| `emotion()` | `&EmotionPrompts` | Emotion-tag output rule text, token/example headers, per-emotion examples, and `natural_dialogue_contract`. |
| `memory()` | `&MemoryPrompts` | Episodic memory recall templates. Has `render_summary_item(age, text)` and `render_facts_header(user_name)`. |
| `summarizer()` | `&SummarizerPrompts` | LLM summarizer system/user prompt templates. Has `render_system(user_name, char_name, existing_facts, conversation)` and `render_user_prompt(user_name, existing_facts, conversation)`. |
| `split()` | `&SplitPrompts` | Session-split reason message templates (`reason_timeout`, `reason_topic`, `reason_context`, `reason_composite`, `reason_manual`). Has `render_reason_timeout(minutes)`, `render_reason_topic(similarity)`, `render_reason_composite(score)`. |
| `extractor()` | `&ExtractorPrompts` | LLM memory-extractor system/user prompt templates. Has `render_user_prompt(conversation, pattern_hints)`. |
| `affect_classifier()` | `&AffectClassifierPrompts` | LLM affect-classifier system/user prompt templates. Has `render_user_prompt(current_affect, conversation)`. |
| `proactive()` | `&ProactivePrompts` | Proactive speech gate, generation-hint, and screen-summary templates. Has `render_generation_hint(topic_hint)`. |

### `substitute`

```rust
pub fn substitute(template: &str, vars: &[(&str, &str)]) -> String
```

Replaces every `{name}` placeholder in `template` with the matching
value from `vars`. Unknown placeholders are left untouched (no panic,
no error). This is the primitive every `render_*` helper above is built
on; re-exported at the crate root as `substitute_prompt_vars` to avoid
colliding with other `substitute` names.

```rust,no_run
use ene_config::PromptLibrary;

let lib = PromptLibrary::load("en");
let framing = lib.system().render_mascot_context("Alicia", "Sam");
let facts_header = lib.memory().render_facts_header("Sam");
```

---

## Usage Example

### Loading and mutating `EneConfig` directly

```rust,no_run
use ene_config::{load_config, save_full_config, EneConfigError};

fn update_model(new_model: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Load with default paths; propagates EneConfigError on a malformed settings.json.
    let mut config = load_config()?;

    // Read a section (returns T::default() if the key is missing)
    let mut llm: LlmConfig = config.get_section().unwrap_or_default();
    println!("Using model: {}", llm.model);

    // Modify and write back
    llm.model = new_model.to_string();
    config.set_section(&llm).map_err(EneConfigError::from)?;
    save_full_config(&config)?;
    Ok(())
}
```

### Using `ConfigStore` (preferred for long-running processes)

```rust,no_run
use ene_config::ConfigStore;

fn update_model_via_store(store: &ConfigStore, new_model: &str) {
    store.with_config_mut(|_config| {
        // In practice, mutate a typed section instead of raw EneConfig fields.
    });

    let mut llm: LlmConfig = store.get_section();
    llm.model = new_model.to_string();
    store.set_section(&llm);

    // Called once per frame / tick in the real host; forced here for the example.
    let _ = store.flush(None);
}
```

---

## Adding a New Config Field

Follow **Recipe R2** from `AGENTS.md`:

1. Edit the relevant struct in `crates/ene-config/src/config.rs` using `define_config!`.
2. Run `cargo run -p ene-cli` once to regenerate `assets/settings.schema.json`.
3. Document the new field in `docs/reference/configuration/settings.md` and `docs/ja/reference/configuration/settings.md`.

---

## See Also

- [`ene-runtime`](./ene-runtime.md) — Consumes `EneConfig` at runtime
- [Configuration Guide](../configuration/settings.md) — End-user settings reference
