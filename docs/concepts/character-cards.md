# Character cards

A character card is the definition of one character: who they are, how they
talk, what they know, and how they look and move. Ene implements the
[Character Card Spec V3](https://github.com/kwaroran/character-card-spec-v3)
and adds an Ene-specific extension block.

## Card formats and import

Ene loads cards from three containers and normalizes them into a folder
under `assets/characters/<name>/`:

| Format | What it is | How it lands on disk |
|---|---|---|
| Folder form | `character.json` + `avatar.png` + asset files | The native form; edit in place |
| PNG card | A PNG whose text chunks (`ccv3` for V3, `chara` for V2) embed the card JSON | Imported as a folder; the PNG is kept as `avatar.png` |
| CHARX | A ZIP archive containing card JSON + assets (VRM, VRMA, images) | Extracted entry-by-entry into a folder |

Import never overwrites an existing character folder. `ene-card` accepts PNG
and CHARX. There is no `ene-ctl import` command yet — drop a folder under
`assets/characters/<name>/`, or call `ene_card` from a host. Plain JSON
files are not imported — they belong directly in a character folder.

Import sizes are validated (per-entry and total archive caps) so a hostile
archive cannot exhaust disk or memory.

## What a card contains

The core `data` object (from the V3 spec):

| Field | Meaning |
|---|---|
| `name`, `nickname` | Character names; `nickname` wins when set |
| `description`, `personality`, `scenario` | Who/what/where — compiled into the identity kernel |
| `system_prompt`, `post_history_instructions` | System instructions; PHI is appended after history |
| `first_mes`, `alternate_greetings` | Opening messages |
| `mes_example` | Example dialogue shown on the first turn |
| `character_book` | The lorebook (see below) |
| `authors_note`, `authors_note_depth` | Persistent instruction injected at a depth in history |
| `creator_notes`, `tags`, `source`, `creation_date` | Provenance/metadata |
| `assets` | References to external files (VRM, VRMA, icons) via `ccdefault:` URIs |
| `extensions.ene` | Ene-specific block (see below) |

Unknown card fields are preserved on edit-and-save, so cards produced by
other tools round-trip without data loss.

## The `extensions.ene` block

This is where Ene-specific behaviour lives:

| Key | Meaning |
|---|---|
| `expressions` | Named expression (blend-shape) definitions the avatar can show |
| `motion_catalog` | Named motion clips (VRMA) grouped into layers |
| `affect_baseline` | Resting PAD affect that emotion decay converges toward |
| `speech` | Speech-style definition (length, politeness) rendered into the identity kernel |
| `ng_expressions` | Phrases the character must never say (output contract) |
| `style_examples` | Situation-labeled response examples |
| `relationship_stages` | Affinity-gated speaking tones |
| `time_periods` | Local-time-gated behaviours (morning/evening/night) |
| `scene_behaviors` | Keyword-gated behaviours for the active scene |
| `locales` | Per-language card diffs (PNG-distributed cards) |

## CBS macros

Card text is expanded with Character Book Spec template macros before it
reaches the model:

| Macro | Expansion |
|---|---|
| `{{char}}`, `<char>`, `<bot>` | Character name |
| `{{user}}` | User name |
| `{{random:a,b,c}}` | Random pick, re-rolled every evaluation |
| `{{pick:a,b,c}}` | Stable pick for the session |
| `{{roll:d20}}` | Dice roll 1..N |
| `{{reverse:text}}` | Reversed text |
| `{{comment:...}}`, `{{//...}}` | Removed |
| `{{description}}`, `{{personality}}`, `{{scenario}}` | Card fields |
| `{{persona}}` | User persona text |
| `{{user_persona}}` | Structured user persona fields |
| `{{date}}`, `{{time}}`, `{{isodate}}`, `{{isotime}}`, `{{weekday}}` | Current time |
| `{{idle_duration}}` | Time since last user activity |

`{{pick}}` uses a per-session seed so the same option is chosen on every
turn of a chat, while `{{random}}` re-rolls.

## Lorebook

The lorebook (`character_book`) is a list of entries with keywords and
content. An entry activates when its keywords match the conversation. Ene
distinguishes two injection positions:

- `before_char` — guaranteed entries placed before the identity kernel.
- `after_char` (default) — guaranteed entries after the character
  description; remaining matched entries flow into the semantic context
  section.

In addition to per-turn injection, lorebook content is **synced into the
memory store as semantic memories** (with embeddings) when the card is
loaded, so related entries can be recalled even when their keywords are not
present verbatim.

## Persona formats

`personality`/`description` text in pseudo-structured formats is parsed
into dense attribute lines (the identity kernel keeps the content and drops
the format syntax):

- **W++** — `[character("Name"){Attribute("value")…}]` blocks.
- **AliChat** — `Key: value` text using the standard AliChat key set.
- **YAML** — flat `key: value` mappings.

Unrecognized text is used verbatim.

## Localization

A card can ship localized variants:

- Folder/CHARX cards: a `character.<lang>.json` sidecar next to the card
  containing only the localized fields.
- PNG cards: an `extensions.ene.locales` bag embedded in the card.

The active locale is chosen from the stage language /
`character_settings.json` `language` override, and the diff is layered over
the base card.

## Per-character presentation

`character_settings.json` next to the card can store stage presentation:
model position/scale, look-at strength, default motion, default expression,
and card language. See [Character editor](../guides/character-editor.md).
