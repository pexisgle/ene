# Character editor

There is no in-process Settings form. Edit the package or V3 fixture, then
let the daemon reload.

## 1. Motions and expressions

Canonical packages use soul/body files under
`<data_dir>/characters/<id>@<version>/`. The V3 sample
`assets/characters/Alicia/character.json` uses `extensions.ene`:

```json
{
  "extensions": {
    "ene": {
      "motion_catalog": {
        "motions": [
          { "name": "wave", "path": "motions/wave.vrma" }
        ]
      },
      "expressions": [
        { "name": "smile", "vrm": { "happy": 0.8 }, "enabled": true }
      ]
    }
  }
}
```

Put `.vrma` files next to the card and reference them with `path`.
`vrm` maps blend-shape names on the model. Localize with
`character.ja.json` beside the card.

## 2. Per-character presentation

`character_settings.json` can store stage placement (position, scale,
look-at, default motion / expression, language). Stage reads that file.

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

`default_motion` must match `motion_catalog.motions`; `default_expression`
must match `expressions` (or a VRM built-in such as `neutral`).

## 3. Import

`ene-card` / `POST /api/v1/characters/import` load PNG (ccv3/chara) and
CHARX. Import writes a package under the data directory and never
overwrites an existing install. The Alicia folder is a V3 fixture for
local editing, not the install path.

`character.schema.json` in `assets/schema/` (regenerated at config init)
validates card JSON in editors.
