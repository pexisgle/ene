# Character Card Assets & Import

CCv3 cards can declare external assets in `data.assets` (VRM models, VRMA
motions, icons, backgrounds, ...). This page documents how Ene consumes
asset declarations, which URI schemes are supported, and how PNG / CHARX
cards are imported.

## Declared assets (`x_vrm` / `x_vrma`)

The spec reserves `x_`-prefixed types for application-specific assets. Ene
consumes:

- `x_vrm` — VRM 1.0 model files used as the 3D avatar;
- `x_vrma` — VRMA motion clips selectable in the desktop UI.

Other types (`icon`, `background`, `emotion`, ...) are parsed but not
consumed yet. The unprefixed `vrm` / `vrma` spellings are also accepted for
cards produced by other tools.

When a card declares `assets`, desktop discovery resolves VRM / motion paths
from those declarations (declaration order is preserved). A declared asset
that is missing on disk is skipped and surfaces as a warning in the desktop
UI. Cards **without** `assets` keep the legacy behavior: files are found by
recursive extension scanning of the character folder (symlinks are never
followed). Motion priority is: `extensions.ene.motion_catalog` →
`x_vrma` assets → extension scan.

## URI schemes

`assets[].uri` is validated through one resolver (`resolve_asset_uri`) before
it can influence any file operation. Supported schemes:

- `embeded://path/to/file` — spec spelling (case-sensitive path, `/`
  separators), resolved relative to the card's directory. Traversal
  (`..`), absolute paths, drive prefixes, backslashes, and percent-encoding
  are rejected. The common misspelling `embedded://` is tolerated on input.
- `ccdefault:` — the application default for the type (the bundled
  `AliciaSolid.vrm` / `VRMA_01.vrma`).
- `http://` / `https://` — validated and resolved as remote references.
  Discovery skips them (they are not playable until materialized) and
  imports keep the URI without downloading.
- `data:` — base64 or percent-encoded inline payloads. Discovery skips them;
  imports decode and materialize them (size-capped) into
  `assets/{type}/3d/` and rewrite the card's URI to `embeded://`.
- A value without a scheme is treated as an embedded relative path for
  compatibility with non-spec producers.

Unknown schemes (`file://`, `ftp://`, ...) are ignored per the spec's
MAY-ignore rule; cards remain readable.

## PNG cards (`ccv3` / `chara`)

PNG cards embed the card JSON in a text chunk. Ene reads the `ccv3` chunk
(V3, base64-encoded JSON) and falls back to the legacy `chara` chunk (V2,
base64-encoded JSON), which is wrapped into the V3 shape on load. tEXt
(uncompressed), zTXt, and iTXt (deflate-compressed variants) are supported;
plain JSON values are accepted for producers that skip base64. Chub.ai and
JanitorAI cards work through this path.

## CHARX cards

A CHARX archive is a zip with `card.json` at its root and assets inside.
Entry names are validated before extraction (no traversal, absolute paths,
symlinks, or encrypted entries; per-entry and total size caps), so an
archive cannot write outside the character folder.

## Import

`ene characters import <path>` (or the REPL `/import <path>`) materializes a
PNG or CHARX card as a `characters/{card name}/` folder:

- PNG → `character.json` (from the `ccv3`/`chara` chunk) plus `avatar.png`
  (the original image, so a `ccdefault:` icon resolves to a real file);
- CHARX → full extraction with `card.json` renamed to `character.json`;
- `data:` `x_vrm` / `x_vrma` assets are decoded, written under
  `assets/{type}/3d/`, and the card's URIs are rewritten to `embeded://`;
- unsafe asset URIs (traversal, absolute paths) reject the import;
- an existing target folder rejects the import (no overwrite).

After import the card is a regular character folder, so the desktop and
`ene characters list` discover it on the next scan. `load_character_card`
and `/card` can also read PNG / CHARX files directly without importing.
