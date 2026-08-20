# `ene-config` interface

## Role

Centralized configuration, schema generation, and path resolution. Settings
sections are *declared* here via macros and *owned* by the defining crate.
Character packages live in [`ene-card`](ene-card.md) / `ene-companion`.

## Public modules

| Module | Contents |
|---|---|
| `config` | `EneConfig`, loaders (`load_config`, `load_full_config`, `get_global_config`), `ConfigStore`, `register_config_schema`, `write_schemas`, `HasConfigKey`, `ConfigTarget` |
| `migration` | `CURRENT_CONFIG_VERSION`, `apply_migrations`, `register_migration`, `MigrationFn` |
| `paths` | `assets_dir`, `data_dir`, `app_data_dir`, `config_file_path`, `models_dir`, socket dirs, `IS_DEV_BUILD` |
| `prompts` / `patterns` | `PromptLibrary`, `PatternLibrary`, `SUPPORTED_LANGUAGES`, `resolve_system_language`, `substitute` |
| `resources` / `store` | `ensure_resource_dirs`; `ConfigStore` (dirty tracking, auto-save) |
| `user_persona` | `UserPersona` (structured persona expanded by the `{{user_persona}}` CBS macro) |
| `error` | `ConfigError`, `EneConfigError` |

## Key macros

| Macro | Purpose |
|---|---|
| `define_config!` | Declares a settings section struct (`core`, `harness`, `mind`, `body`, `voice`, `store`, `approval`, `characters`, …), its JSON schema, and its registration (settings / character / nested variants) |
| `define_label_enum!` | Declares a labeled enum with a consistent `label()` API |

Plugin profile rows are fiber state (`ene-fiber`), not a `define_config!`
section.

## Dependencies

- Depends on: nothing internal.
- Used by: every crate and app (sections live in the owning crate).

## Refactoring notes

- **Adding a setting** = a new `define_config!` in the *owning* crate.
- **Removing/renaming a setting** = a migration in `ene-config::migration`
  (bump `CURRENT_CONFIG_VERSION`) plus doc updates.
- `EneConfig` keeps unknown top-level keys on save.
- Generated JSON Schemas under `assets/schema/` are gitignored.
