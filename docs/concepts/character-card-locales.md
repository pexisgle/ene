# Character Card Localization

CCv3 has no multilingual story for the fields the LLM actually reads:
`creator_notes_multilingual` only covers creator notes, which are production
notes for humans and never reach the prompt. Ene localizes cards with
**diff files**: a `character.{lang}.json` next to the base `character.json`
carries only the translatable fields, and the loader layers it over the base
at load time. Untranslated fields fall back to the base language.

```
characters/Alicia/
  character.json        ← base (a complete, valid CCv3 card)
  character.ja.json     ← Japanese diff: only translatable fields
  character_settings.json
  model.vrm
```

## Translatable fields

| Translated | Never translated |
|---|---|
| `description` / `personality` / `scenario` | `assets` |
| `first_mes` / `alternate_greetings` / `mes_example` | `extensions.ene.motion_catalog` |
| `system_prompt` / `post_history_instructions` | `extensions.ene.expressions` (VRM weights) |
| `creator_notes` / `nickname` / `tags` | `creation_date` / `spec` / `spec_version` |
| `character_book` entries' `content`, `keys`, and `secondary_keys` | `insertion_order` / `priority` / `position` |

Lorebook triggers (`keys` and `secondary_keys`) are translated
**mandatorily**: they are matched against conversation text — and Ene's
matcher requires at least one primary AND one secondary key to fire — so an
untranslated trigger never fires in a non-base-language conversation.

The card `name` is deliberately not translatable — it is the character's
identity key used for discovery and folder naming. Only the display-only
`nickname` is. `creator_notes_multilingual` (the CCv3 field) stays supported
for legacy cards, but new cards should use diff files.

## Diff file format

```json
// character.ja.json
{
  "description": "明るいデスクトップコンパニオン。",
  "first_mes": "やっほー、何してるの？",
  "nickname": "アリス",
  "character_book": {
    "entries": [
      {
        "id": "lore-1",
        "keys": ["猫", "ねこ"],
        "secondary_keys": ["ペット"],
        "content": "日本語のロアエントリ。"
      }
    ]
  }
}
```

Every field is optional. `Some` replaces the base value; a missing key keeps
the base language. Lorebook entries are matched against the base card by
`id`; an entry whose id has no match is skipped with a warning, never
appended. `alternate_greetings` and `tags`, when present, replace the whole
base list.

A malformed diff never breaks the card: it is warned about and skipped, and
the base card is returned. Unknown fields are rejected rather than silently
ignored, so a typo like `"first_mess"` makes the whole diff skip with a
warning instead of looking like an untranslated field (a diff written for a
newer Ene that adds fields is skipped by an older Ene the same way).

## How the active locale is chosen

Effective locale = **per-card override** (`character_settings.json`
`language`) → **app language** → system locale. Values are canonicalized
through `resolve_language_alias` (`ja-JP` and `jp` become `ja`, anything
unknown becomes `en`), and the diff file is looked up as
`character.{code}.json` with the canonical code.

- Desktop: the app language is `mind.language`, which the settings screen
  keeps synced with the UI language (`desktop.language`).
- CLI: the app language is the active i18n locale — the `--lang` flag when
  given, else the system-negotiated locale.

The per-card override lives in `character_settings.json`:

```json
{ "language": "ja" }
```

An empty value (the default) inherits the app language. The override applies
to folder-form cards only; CHARX / PNG cards read directly have no settings
file.

## Distribution forms

| Form | Layout |
|---|---|
| Folder (work form) | `character.json` + `character.{lang}.json` sidecars |
| CHARX | `card.json` + `character.{lang}.json` at the zip root |
| PNG | one language merged into the card, or diffs embedded in `extensions.ene.locales.{lang}` |
| Export | base + diff merged into one complete single-language CCv3 card |

The loader normalizes every form to the same in-memory result: a merged
single-language card with the `extensions.ene.locales` bag stripped, so a
PNG-loaded card and a folder-loaded card produce identical bytes on save and
identical memory hashes. The base `load_character_card` (no locale) returns
the base card unchanged — `character.json` alone stays a valid CCv3 card.

Importing a PNG / CHARX card materializes the folder work form: embedded
locales are written out as `character.{lang}.json` sidecars (CHARX-provided
sidecars win) and removed from `character.json`.

`export_character_card` merges base + diff for an explicit language and
writes the complete single-language card with `save_character_card` (atomic).
PNG baking itself is not implemented; the exported JSON is the input a future
baker would embed.

## Language switching at runtime

The card is loaded once at startup (or `/card` in the CLI). Switching the app
language re-loads the card the next time the character is opened; the running
session keeps the card it started with, matching how the runtime already
treats character cards.
