# `ene-card` interface

## Role

Character card containers and import/export: V3 card models (CCv3), PNG/CHARX
container parsing, per-character settings, and localized card diffs. Depends
on `ene-config` only for shared error, path, and language-alias primitives.

## Public modules

| Module | Contents |
|---|---|
| `character_card` | `CharacterCardV3`, `CharacterCardData`, `EneExtension`, `Lorebook`/`LorebookEntry`, expression/affect/speech/relationship types, CBS macro expansion (`expand_cbs_macros*`, `MacroContext`, `session_pick_seed`) |
| `card_import` | `import_character_file`, `ImportedCharacter` (PNG/CHARX → folder) |
| `character_assets` | `ResolvedAssetUri`, `resolve_asset_uri`, `EneAssetKind`, `decode_data_payload`, `DEFAULT_VRM_PATH` |
| `characters` | `discover_characters`, `load_character_card(_localized)`, `export_character_card`, `resolve_character_path`, `CharacterEntry` |
| `character_config` | `CharacterConfig`, `MotionCatalog`, `MotionEntry`, `MotionLayer` |
| `character_store` | `CharacterConfigStore` (dirty tracking, auto-save of `character_settings.json`) |
| `locale` | `LocalizedCharacterFields` and per-field localized types, merge logic |
| root | `save_character_card`, `generate_character_schema_json`, `generate_character_card_schema_json`, `write_character_schemas` |

## Dependencies

- Depends on: `ene-config` (error, paths, language aliases only).
- Used by: `ene-companion` (V3 import).

## Refactoring notes

- `UserPersona` stays in `ene-config`: it is a settings-level type
  (`EneConfig.user_persona`) that the CBS macro consumes.
- Character cards are a shared, externally-specified format (CCv3): the
  `CharacterCardData` shape is constrained by the spec plus `extensions.ene`,
  which is Ene's own extension namespace.
- `card_import` defends against decompression bombs and path traversal
  (`..`) in CHARX archives; keep those guards when touching it.
