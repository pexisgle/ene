# Character Card Editor Guide

The desktop app ships a visual editor for the character card of the currently
selected character (`character.json`, CCv3 format). It is designed so that
users can adjust a card safely without hand-editing JSON: every save runs
schema and asset checks first, writes the file atomically, and keeps a backup
of the original card.

## Opening the editor

1. Launch `ene-desktop` and press `F1` (or the tray menu) to open the settings
   window.
2. Open the **Character Card** tab.

The editor loads the card of the character selected on the **Character** tab.
When you switch characters, the editor reloads for the newly selected one.

## Editing sections

The card is split into collapsible sections, each mapped to a part of the
CCv3 schema:

| Section | Card fields |
|---|---|
| Identity | `data.name`, `data.description`, `data.creator_notes` |
| Personality | `data.personality` |
| Scenario | `data.scenario` |
| Greetings | `data.first_mes`, `data.alternate_greetings` |
| Memory Instructions | `data.system_prompt`, `data.post_history_instructions`, `data.mes_example` |
| Lorebook | `data.character_book.entries` |
| Motion Catalog | `extensions.ene.motion_catalog` |

The lorebook and motion catalog editors expose the fields most cards use
(trigger keys, content, enabled/constant flags, regex/case options, position,
priority, order, comment; motion name, relative path, and body layer).
Fields the editor does not expose — unknown `data` keys, asset declarations,
expression definitions, lorebook `id`/`extensions` — are preserved untouched
when the card is saved.

### Localized cards

The editor always edits the **base-language** card. If the character folder
contains locale diffs (`character.{lang}.json` sidecars), a notice is shown;
those files are not modified.

## Validation before save

Pressing **Validate** checks the assembled card without writing it. Saving
runs the same checks automatically and is **blocked** while any error-level
finding exists. Every finding is shown with the exact field location
(e.g. `data.character_book.entries[0].keys`) so you know which section to
fix.

Checks performed:

- Required fields: `data.name`, non-empty alternate greetings, lorebook
  trigger keys/content, motion names and paths. An empty first message is
  legal CCv3, so it is flagged as a warning rather than blocking the save.
- Motion paths must stay inside the character folder (no `..` or absolute
  paths).
- Declared assets (`data.assets`, VRM/VRMA): the URI must be valid and
  embedded files must exist on disk. A missing VRM blocks saving because it
  would break startup. Remote URLs and data URLs cannot be verified locally
  and produce a non-blocking warning.
- The idle motion must reference a motion in the catalog.
- Lorebook entries: a selective entry without secondary keys and a regex
  trigger key that does not compile are both surfaced as warnings.

## Saving, backup, and discard protection

- **Backup**: the first save of a session copies the pre-edit
  `character.json` to `character.json.bak` next to it. The backup is never
  overwritten, so the original card stays recoverable even after many saves.
- **Atomic write**: the card is written via a temp file + rename, so a crash
  mid-save cannot leave a truncated card.
- **Discard confirmation**: closing the settings window (window close button,
  `Esc`, `F1`), closing the app from the main window, switching characters,
  or pressing **Reload** while there are unsaved changes asks for confirmation
  first. Choose **Discard** to lose the edits or **Keep editing** to go back.
