# `ene-config` Configuration & Card Specifications

The `ene-config` crate provides JSON-based configuration storage, SillyTavern V3 character card deserialization, schema registry hookups, and macro tools for defining type-safe configs.

---

## 1. Top-Level Config Store Methods (`store.rs`)

### `ConfigStore` (Public / Struct)
The in-memory cache and file system manager for Ene configurations.

#### `load`
*   **Signature**: `pub fn load() -> Self`
*   **Description**: Safe fail-hard loader. Attempts to read the config via `try_load`, returning default configurations on errors.

#### `try_load`
*   **Signature**: `pub fn try_load() -> Result<Self, EneConfigError>`
*   **Description**: Connects to the filesystem (`config.json`), parses JSON settings, and maps them to memory cache trees.

#### `from_config`
*   **Signature**: `pub fn from_config(config: EneConfig) -> Self`
*   **Description**: Wraps raw configuration structs into stores.

#### `config`
*   **Signature**: `pub fn config(&self) -> EneConfig`
*   **Description**: Clones the active configuration.

#### `with_config_mut`
*   **Signature**: `pub fn with_config_mut(&self, f: impl FnOnce(&mut EneConfig))`
*   **Description**: Updates configurations in-place and sets dirty flags.

#### `set_config`
*   **Signature**: `pub fn set_config(&self, config: EneConfig)`
*   **Description**: Replaces configurations and flags the store as dirty.

#### `get_section`
*   **Signature**: `pub fn get_section<T>(&self) -> T where T: serde::de::DeserializeOwned + Default + crate::HasConfigKey`
*   **Description**: Extracts sub-sections from configurations.

#### `set_section`
*   **Signature**: `pub fn set_section<T>(&self, section: &T) where T: serde::Serialize + crate::HasConfigKey`
*   **Description**: Serializes and sets configuration sub-sections.

#### `character_config`
*   **Signature**: `pub fn character_config(&self) -> CharacterConfig`
*   **Description**: Returns character sub-configurations.

#### `load_character_config`
*   **Signature**: `pub fn load_character_config(&self, character_name: &str)`
*   **Description**: Loads character configurations from files.

#### `set_character_config`
*   **Signature**: `pub fn set_character_config(&self, config: CharacterConfig)`
*   **Description**: Sets active character configurations.

#### `get_character_section` / `set_character_section`
*   **Signature**: `pub fn get_character_section<T>(&self) -> T ...` (same pattern for sets)
*   **Description**: Accesses/modifies sub-sections inside character configurations.

#### `flush_if_dirty`
*   **Signature**: `pub fn flush_if_dirty(&self, character_name: Option<&str>) -> std::io::Result<bool>`
*   **Description**: Writes changes back to files only if dirty flags are set.

#### `flush`
*   **Signature**: `pub fn flush(&self, character_name: Option<&str>) -> std::io::Result<()>`
*   **Description**: Forces saving settings back to files.

#### `is_dirty`
*   **Signature**: `pub fn is_dirty(&self) -> bool`
*   **Description**: Returns `true` if changes are pending.

---

## 2. Configuration & Card Loading (`config.rs` & `character_card.rs`)

#### `update_global_config` / `get_global_config`
*   **Signature**: `pub fn update_global_config(config: EneConfig)` (same pattern for getters)
*   **Description**: Reads/writes thread-safe global settings configurations.

#### `__register_schema` / `__register_tool_schema`
*   **Signature**: `pub fn __register_schema<T: JsonSchema + HasConfigKey>(target: ConfigTarget, parent_key: Option<&str>)`
*   **Description**: Links JSON validation structures for automated CLI schema exports.

#### `EneConfig::get_section` / `set_section`
*   **Signature**: `pub fn get_section<T>(&self) -> Result<T, EneConfigError> ...`
*   **Description**: Accesses or modifies configuration categories.

#### `generate_schema_json` / `generate_character_schema_json`
*   **Signature**: `pub fn generate_schema_json() -> Result<String, serde_json::Error>`
*   **Description**: Generates unified validation schemas.

#### `load_character_card`
*   **Signature**: `pub fn load_character_card(name_or_path: &str) -> Result<crate::CharacterCardV3, crate::EneConfigError>`
*   **Description**: Parses PNG metadata blocks or JSON models for character configurations.

#### `expand_cbs_macros`
*   **Signature**: `pub fn expand_cbs_macros(text: &str, char_name: &str, user_name: &str) -> String`
*   **Description**: Replaces `{{char}}` and `{{user}}` macro templates in prompts.

#### `resolve_expressions`
*   **Signature**: `pub fn resolve_expressions(card: &CharacterCardV3) -> Vec<ResolvedExpression>`
*   **Description**: Extracts visual expression configurations from character cards.

---

## 3. Dynamic Configuration Macros

#### `define_config!` / `define_tool_config!`
*   **Description**: Macros that declare configurations, derive serializations, and register schemas statically.

---

## 4. Text Truncation Utilities (`truncate.rs`)

#### `Truncate::chars`
*   **Signature**: `pub fn chars(text: &str, max_chars: usize) -> String`
*   **Description**: Truncates text to fit within character limits.

#### `Truncate::simple`
*   **Signature**: `pub fn simple(text: &str, max_chars: usize) -> String`
*   **Description**: Appends ellipses (`...`) if text exceeds limits.

#### `Truncate::detailed`
*   **Signature**: `pub fn detailed(text: &str, max_chars: usize) -> String`
*   **Description**: Appends metadata (e.g. `[truncated 200 chars]`) if text exceeds limits.

#### `Truncate::output` / `Truncate::tail`
*   **Signature**: `pub fn output(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult`
*   **Description**: Truncates text by line or byte limits.

---

## 5. Path Resolvers (`paths.rs`)

#### `app_data_dir`
*   **Signature**: `pub fn app_data_dir() -> PathBuf`
*   **Description**: Returns Ene's configuration path (`~/.gemini/antigravity/`).

#### `assets_dir`
*   **Signature**: `pub fn assets_dir() -> PathBuf`
*   **Description**: Returns Ene's assets path.

#### `models_dir`
*   **Signature**: `pub fn models_dir() -> PathBuf`
*   **Description**: Returns Ene's local GGUF models path.

#### `config_file_path`
*   **Signature**: `pub fn config_file_path() -> PathBuf`
*   **Description**: Returns Ene's `config.json` path.

#### `tool_socket_dir`
*   **Signature**: `pub fn tool_socket_dir() -> PathBuf`
*   **Description**: Returns IPC socket directories.
