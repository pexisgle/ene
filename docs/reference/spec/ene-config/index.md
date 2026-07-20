# `ene-config` Configuration & Card Specifications

The `ene-config` crate provides JSON-based configuration storage, SillyTavern V3 character card deserialization, schema registry hookups, and macro tools for defining type-safe configs.

---

## 1. Core Data Structures

### `EneConfig` (Public / Struct)
The top-level configuration object. Holds JSON mappings of different runtime sections (`ai`, `store`, `mind`, `tools`, `desktop`), deserializable on-demand via `get_section::<T>()`.

### `ConfigStore` (Public / Struct)
The disk manager for `config.json`.
*   `load() -> Result<Self, ConfigError>`: Loads settings from disk, writing defaults if not found.
*   `save(&self) -> Result<(), ConfigError>`: Serializes changes back to disk (checking a `dirty` flag to prevent redundant writes).

### `CharacterCardV3` (Public / Struct)
Tavern-compatible Character Card V3 model holding identity prompts, character descriptions, lorebook entries, asset links, and expression blendshape definitions.

---

## 2. Dynamic Schema Registration Macros

Ene registers validation schemas at boot time using declarative macros to prevent syntax mismatches in `config.json`:

### `define_config!`
*   **Syntax**:
    ```rust
    define_config!(
        settings,
        "mind.emotion",
        pub struct EmotionConfig {
            pub enabled: bool = true,
            pub decay_half_life_minutes: f64 = 60.0,
        }
    );
    ```
*   **Expansion Details**:
    -   Automatically derives `Serialize`, `Deserialize`, and `JsonSchema` (using schemars).
    -   Implements `Default` mapping field values to the inline `= default_value` syntax.
    -   **Static ctor registration**:
        Leverages the `ctor` crate to run code statically before `main()` executes, invoking `__register_schema` to register the struct's validation schema under the configured key path.

### `define_tool_config!`
*   Declares configuration structures for tools, generating validation schemas registered in the host tool server.

---

## 3. Safe Character Truncation (`Truncate`)

A utility to truncate user and assistant messages to prevent CJK (Chinese, Japanese, Korean) encoding splits:

*   `Truncate::truncate_chars(&self, max_chars: usize) -> TruncateResult`:
    -   Finds UTF-8 character boundaries (Unicode Scalar Values), cutting strings at clean points rather than byte limits to avoid invalid sequences.
