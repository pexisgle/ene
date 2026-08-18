# Character editor

There is no in-process Settings form. Edit the card files, then let the
daemon reload the character package.

## 1. The card file

The card is `character.json` (V3 spec + `extensions.ene`). See
[Character cards](../concepts/character-cards.md) for the field reference.

Useful edits:

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

## 2. Per-character presentation

`character_settings.json` next to the card can store stage presentation
(position, scale, look-at, default motion / expression, language). Stage
reads that file; `ene-core` does not own a second copy.

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

`default_motion` must match `extensions.ene.motion_catalog`;
`default_expression` must match `extensions.ene.expressions` (or a VRM
built-in such as `neutral`).

## 3. Import a card from elsewhere

`ene-card` loads PNG (ccv3/chara chunks) and CHARX (zip). Import never
overwrites an existing character folder. There is no `ene-ctl import`
command yet — place a folder under `assets/characters/<name>/` or call
`ene_card` from a host.

`character.schema.json` in `assets/schema/` (regenerated at config init)
validates card JSON in editors.
