# `ene-config` interface

## Role

Centralized configuration, schema generation, and path resolution. Every
crate's settings sections are *defined* here via macros but *owned* by the
defining crate. Character card containers live in
[`ene-card`](ene-card.md).

## Public modules

| Module | Contents |
|---|---|
| `config` | `EneConfig`, loaders (`load_config`, `load_full_config`, `get_global_config`), `ConfigStore`, `register_config_schema`, `write_schemas`, `HasConfigKey`, `ConfigTarget` |
| `migration` | `CURRENT_CONFIG_VERSION`, `apply_migrations`, `register_migration`, `MigrationFn` |
| `paths` | `assets_dir`, `app_data_dir`, `config_file_path`, `models_dir`, socket dirs, `IS_DEV_BUILD` |
| `prompts` / `patterns` | `PromptLibrary`, `PatternLibrary`, `SUPPORTED_LANGUAGES`, `resolve_system_language`, `substitute` |
| `resources` / `store` | `ensure_resource_dirs`; `ConfigStore` (dirty tracking, auto-save) |
| `user_persona` | `UserPersona` (structured persona expanded by the `{{user_persona}}` CBS macro) |
| `error` | `ConfigError`, `EneConfigError` |

## Key macros

| Macro | Purpose |
|---|---|
| `define_config!` | Declares a settings section struct, its JSON schema, and its registration (settings / character / nested variants) |
| `define_label_enum!` | Declares a labeled enum with a consistent `label()` API |

## Dependencies

- Depends on: nothing internal.
- Used by: every crate and app (config sections live in the owning crate,
  e.g. `ene-session`, `ene-kernel`, `ene-companion`, `ene-plane`).

## Refactoring notes

- **Adding a setting** = a new `define_config!` in the *owning* crate; the
  schema registry picks it up automatically at startup.
- **Removing/renaming a setting** = a settings migration in
  `ene-config::migration` (bump `CURRENT_CONFIG_VERSION`) plus doc updates.
- `EneConfig` keeps unknown top-level keys on save (round-trip safety) —
  preserve that behaviour when restructuring the config surface.
- The generated JSON Schemas under `assets/schema/` are build artifacts
  (gitignored); never hand-edit them.
- `UserPersona` is a settings-level type (`EneConfig.user_persona`), not a
  card container type; it stays here while the CBS macro machinery that
  consumes it lives in `ene-card`.
