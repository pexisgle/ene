# Third-party assets

The root [`LICENSE`](LICENSE) covers ene source code and the project-owned icon
only. It does not override the terms embedded in or published for a model,
motion, or font. The machine-readable inventory in
[`third_party/assets.json`](third_party/assets.json) is the source of truth for
file hashes, provenance, permissions, and distribution paths.

## Inventory

| Asset | Publisher | License / terms | Distribution status |
| --- | --- | --- | --- |
| `assets/characters/Alicia/AliciaSolid.vrm` | DWANGO Co., Ltd. | VRM Platform License 1.0 plus the [Niconi 3D-chan license](https://3d.nicovideo.jp/alicia/rule.html) | Not distributed with this repository; obtain locally via [`scripts/fetch-character-assets.sh`](scripts/fetch-character-assets.sh). Embedded metadata disallows redistribution and modification |
| `assets/characters/Alicia/motions/VRMA_01.vrma` through `VRMA_07.vrma` | pixiv Inc. / VRoid Project | [VRM Animation 7-piece set terms](https://booth.pm/ja/items/5512385) | Not distributed with this repository; obtain locally via [`scripts/fetch-character-assets.sh`](scripts/fetch-character-assets.sh). Commercial use requires the published attribution and extractable-file redistribution is prohibited |
| `assets/fonts/NotoSansJP-Regular.ttf` | Noto Project / Google Fonts | [SIL Open Font License 1.1](https://github.com/google/fonts/raw/main/ofl/notosansjp/OFL.txt) | Development fixture only; the family and license are identified, but the exact upstream revision is not hash-matched |
| `assets/icon.png` | ene contributors | MIT, as a project-owned asset | Included in the Debian package with `LICENSE` |

The Alicia and VRMA files are not checked into this repository: their terms
prohibit redistributing them as standalone assets, and a public source tree
counts as redistribution. Each user obtains them directly from the rights
holder by following [`scripts/fetch-character-assets.sh`](scripts/fetch-character-assets.sh),
which prints the manual agreement steps and verifies any placed files against
the hashes recorded in this inventory. They must never be copied into a
release artifact or committed back to this repository; local copies are
covered by [.gitignore](.gitignore). A provenance entry is not a license grant.

## Required attribution

When the VRMA files are used under the published terms, include this exact
attribution:

> Character animation credits to pixiv Inc.'s VRoid Project

The Alicia model requires the credit notation declared by its embedded VRM
metadata and its official license page. The official terms control if this
notice and the embedded metadata differ.

## Verification

Run `bash scripts/check-asset-provenance.sh` from the repository root. The
check compares the tracked binary/media inventory with the manifest and
recomputes every SHA-256 hash. Entries marked `distribution: "local-only"`
describe files that stay out of git; they are verified only when placed, and
missing ones are accepted. A binary replacement requires a new provenance
review; changing only the manifest hash is not sufficient.
