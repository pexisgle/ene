# `ene-config` — API Reference

> **Crate:** `ene-config`  
> **Role:** Configuration loading, schema generation, and character card types for the Ene system.

---

## Overview

`ene-config` manages all runtime configuration using the [`figment`](https://docs.rs/figment) layered configuration system. It defines the `EneConfig` type, the `define_config!` macro for declaring typed sections, global config state, and the character card format.

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
pub fn load_config() -> EneConfig
```

Loads configuration using the default paths (`assets/` directory next to the binary). This is the primary entry point for application startup. Internally calls `load_full_config()`.

### `load_config_from`

```rust
pub fn load_config_from(assets_dir: &Path, config_path: &Path) -> EneConfig
```

Loads configuration from an explicit directory and file path. Useful for tests or non-standard deployments. Internally calls `load_full_config_from`.

### `load_full_config` / `load_full_config_from`

```rust
pub fn load_full_config() -> EneConfig
pub fn load_full_config_from(assets_dir: &Path, config_path: &Path) -> EneConfig
```

Full `EneConfig` load. Reads `settings.json`, applies `ENE_*` environment
variables, writes generated schemas to the assets dir, and updates the
global singleton via `update_global_config`.

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
adds dirty tracking and auto-save.

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

## Character Config

`ene_config::CharacterConfig` is the per-character companion to
`EneConfig`. It is loaded from `character_settings.json` next to the
character card and holds UI/runtime preferences such as
`position`, motion presets (`MotionEntry`), and expression bindings.

```rust
pub struct CharacterConfig { /* … */ }
pub struct MotionEntry { /* … */ }
```

---

## Usage Example

```rust
use ene_config::{load_config, EneConfig, EneConfigError};

// Load with default paths
let config = load_config();

// Read a section (returns T::default() if the key is missing)
let llm: LlmConfig = config.get_section().unwrap_or_default();
println!("Using model: {}", llm.model);

// Modify and write back
let mut config = config;
let mut llm = config.get_section::<LlmConfig>().unwrap_or_default();
llm.model = "gpt-4o-mini".to_string();
config.set_section(&llm).map_err(EneConfigError::from)?;
ene_config::save_full_config(&config)?;
```

---

## Adding a New Config Field

Follow **Recipe R2** from `AGENTS.md`:

1. Edit the relevant struct in `crates/ene-config/src/config.rs` using `define_config!`.
2. Run `cargo run -p ene-cli` once to regenerate `assets/settings.schema.json`.
3. Document the new field in `docs/configuration/settings.md` (English and Japanese).

---

## See Also

- [`ene-core`](./ene-core.md) — Consumes `EneConfig` at runtime
- [Configuration Guide](../configuration/settings.md) — End-user settings reference
