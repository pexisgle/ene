# Character packages

A companion is a **soul** (identity) plus an optional **body** (VRM / motion)
bound as a character package. Canonical installs live under
`<data_dir>/characters/<id>@<version>/`. The field contract is rustdoc on
`ene-companion` / `ene-card`.

## Formats

| Format | Role |
|---|---|
| `.enechar` / `.enesoul` / `.enebody` | Canonical packages. Import with `POST /api/v1/characters/import`. |
| Character Card V3 (folder, PNG `ccv3`, CHARX zip) | Import only. Converted into a package under the data directory. |

`assets/characters/Alicia/` is a V3 **dev fixture**, not the runtime layout.
Import never overwrites an existing install. Size caps apply per entry and
for the whole archive.

V3 `data` fields (`name`, `description`, `personality`, …) and
`extensions.ene` (expressions, `motion_catalog.motions`, speech) are mapped
on import. Unknown fields round-trip in `ene-card`.

## Lorebook and templates

Lorebook data is preserved for import/export compatibility, but Ene does not
interpret, search, or inject it into model context. Stage v1 has no runtime
lorebook editor. Card text may use Character Book Spec macros (`{{char}}`,
`{{user}}`, `{{random:…}}`, `{{date}}`, …). Persona text in W++ / AliChat /
YAML is flattened into the identity kernel.

Folder cards may ship `character.<lang>.json` sidecars. Stage presentation
(`character_settings.json`: position, scale, default motion / expression)
stays next to a V3 fixture or in the installed package.

See [Character editor](../guides/character-editor.md).
