# `ene-config` interface

## Role

Centralized configuration, schema generation, character cards, and path
resolution. Every crate's settings sections are *defined* here via macros
but *owned* by the defining crate.

## Public modules

| Module | Contents |
|---|---|
| `config` | `EneConfig`, loaders (`load_config`, `load_full_config`, `get_global_config`), `ConfigStore`, `register_config_schema`, `write_schemas`, `HasConfigKey`, `ConfigTarget` |
| `character_card` | `CharacterCardV3`, `CharacterCardData`, `EneExtension`, `Lorebook`/`LorebookEntry`, `UserPersona`, expression/affect/speech/relationship types, CBS macro expansion (`expand_cbs_macros*`, `MacroContext`, `session_pick_seed`) |
| `card_import` | `import_character_file`, `ImportedCharacter` (PNG/CHARX → folder) |
| `character_assets` | `ResolvedAssetUri`, `resolve_asset_uri`, `EneAssetKind`, `decode_data_payload`, `DEFAULT_VRM_PATH` |
| `characters` | `discover_characters`, `load_character_card(_localized)`, `export_character_card`, `resolve_character_path`, `CharacterEntry` |
| `character_config` | `CharacterConfig`, `MotionCatalog`, `MotionEntry`, `MotionLayer` |
| `locale` | `LocalizedCharacterFields` and per-field localized types, merge logic |
| `migration` | `CURRENT_CONFIG_VERSION`, `apply_migrations`, `register_migration`, `MigrationFn` |
| `paths` | `assets_dir`, `app_data_dir`, `config_file_path`, `models_dir`, socket dirs, `IS_DEV_BUILD` |
| `prompts` / `patterns` | `PromptLibrary`, `PatternLibrary`, `SUPPORTED_LANGUAGES`, `resolve_system_language`, `substitute` |
| `resources` / `store` | `ensure_resource_dirs`; `ConfigStore` (dirty tracking, auto-save) |
| `error` | `ConfigError`, `EneConfigError` |

## Key macros

| Macro | Purpose |
|---|---|
| `define_config!` | Declares a settings section struct, its JSON schema, and its registration (settings / character / nested variants) |
| `define_tool_config!` | Declares a tool's config schema under `tools.list.<tool>` |
| `define_label_enum!` | Declares a labeled enum with a consistent `label()` API |

## Dependencies

- Depends on: nothing internal.
- Used by: every crate and app (config sections live in the owning crate,
  e.g. `ene-ai::AiConfig`, `ene-mind::MindConfig`, `ene-store::StoreConfig`,
  `ene-plugin-host::PluginConfig`, `apps/ene-desktop`).

## Refactoring notes

- **Adding a setting** = a new `define_config!` in the *owning* crate; the
  schema registry picks it up automatically at startup.
- **Removing/renaming a setting** = a settings migration in
  `ene-config::migration` (bump `CURRENT_CONFIG_VERSION`) plus doc updates.
- `EneConfig` keeps unknown top-level keys on save (round-trip safety) —
  preserve that behaviour when restructuring the config surface.
- The generated JSON Schemas under `assets/schema/` are build artifacts
  (gitignored); never hand-edit them.
- Character cards are a shared, externally-specified format (CCv3): the
  `CharacterCardData` shape is constrained by the spec plus `extensions.ene`,
  which is Ene's own extension namespace.
