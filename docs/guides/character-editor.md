# Character editor

You can edit a character at three levels, from "no files touched" to "full
control":

## 1. Desktop character editor

Settings → Character editor edits the active card's fields through a form:
name, description, personality, scenario, first message, system prompt,
post-history instructions, example dialogue, and the Ene-specific
expressions/motions. Changes are saved back to the card's `character.json`
and take effect on the next turn.

## 2. Per-character presentation settings

`assets/characters/<name>/character_settings.json` controls the desktop
scene (see [Configuration → Per-character settings](../configuration.md#per-character-settings-character_settingsjson)):

```json
{
  "character_position": [0.0, 0.0, 0.0],
  "model_scale": 1.0,
  "look_at_strength": 0.6,
  "default_motion": "idle",
  "default_expression": "neutral",
  "language": ""
}
```

`default_motion` must match an entry in the card's
`extensions.ene.motion_catalog`; `default_expression` must match an entry
in `extensions.ene.expressions` (or a VRM built-in like `neutral`).

## 3. The card file itself

The card is `character.json` (V3 spec + `extensions.ene`). See
[Character cards](../concepts/character-cards.md) for the full field
reference. Useful editing patterns:

- **Add a motion** — put the `.vrma` file in the character folder, then
  add a `motion_catalog` entry referencing it:

```json
{
  "extensions": {
    "ene": {
      "motion_catalog": {
        "entries": [
          { "name": "wave", "file": "wave.vrma", "layer": "overlay" }
        ]
      }
    }
  }
}
```

- **Add an expression** — define a blend-shape expression with a target
  name from the VRM model:

```json
{
  "extensions": {
    "ene": {
      "expressions": [
        { "name": "smile", "target": "happy", "weight": 0.8 }
      ]
    }
  }
}
```

- **Localize the card** — add `character.ja.json` (or
  `character.en.json`) next to the card with only the translated fields.

## 4. Import a card from elsewhere

```sh
ene --character "" import /path/to/card.png
# or in the REPL:
/import /path/to/card.charx
```

Imports never overwrite an existing character folder. After importing,
switch to the new character with `/card <name>` or Settings → Character.

## Validating changes

- `/characters` lists discovered characters and their card paths.
- `/card <name>` reloads a card in the running CLI.
- `/doctor` checks config and store health.
- The `character.schema.json` in `assets/schema/` validates card JSON in
  editors (regenerated at startup).
