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

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `get_section` | `fn get_section<T: ConfigSection>(&self) -> Option<T>` | Returns the configuration section for type `T`, or `None` if not present. |
| `set_section` | `fn set_section<T: ConfigSection>(&mut self, section: &T)` | Replaces or inserts the configuration section for type `T`. |

---

## `define_config!` Macro

The `define_config!` macro declares a typed configuration section. Each section is independently serializable, deserializable, and has a unique registry key.

```rust
define_config! {
    /// LLM backend configuration.
    #[section = "llm"]
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
1. Derives `serde::Serialize`, `serde::Deserialize`, `Clone`, and `Debug`.
2. Implements `ConfigSection` with the given `#[section]` key.
3. Generates a JSON Schema and registers it with `register_runtime_schema`.

---

## Global Config Functions

### `load_config`

```rust
pub fn load_config() -> EneConfig
```

Loads configuration using the default paths (`assets/` directory next to the binary). This is the primary entry point for application startup.

### `load_config_from`

```rust
pub fn load_config_from(assets_dir: &Path, config_path: &Path) -> EneConfig
```

Loads configuration from an explicit directory and file path. Useful for tests or non-standard deployments.

### `register_runtime_schema`

```rust
pub fn register_runtime_schema(key: &str, schema: serde_json::Value)
```

Registers a JSON Schema fragment under `key`. Called automatically by `define_config!` for each section. Applications should not need to call this directly.

### `write_schemas`

```rust
pub fn write_schemas(assets_dir: &Path)
```

Writes the collected JSON Schema fragments to `settings.schema.json` and `character_settings.schema.json` in `assets_dir`. Called during CLI startup to keep schemas in sync.

### `resolve_character_path`

```rust
pub fn resolve_character_path(name: &str) -> String
```

Resolves a character name to a filesystem path, searching the configured character directories.

---

## Global Config Singleton

```rust
pub static GLOBAL_CONFIG: OnceLock<RwLock<EneConfig>>;
```

A process-global `RwLock<EneConfig>` initialized on first use. The actor reads this on startup; use `EneHandle::reconfigure` to update it at runtime.

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

## Usage Example

```rust
use ene_config::{load_config, EneConfig};

// Load with default paths
let config = load_config();

// Read a section
if let Some(llm) = config.get_section::<LlmConfig>() {
    println!("Using model: {}", llm.model);
    println!("API base: {}", llm.api_base);
}

// Modify and write back
let mut config = config;
let mut llm = config.get_section::<LlmConfig>().unwrap_or_default();
llm.model = "gpt-4o-mini".to_string();
config.set_section(&llm);
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
