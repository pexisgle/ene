# Bundled assets

This directory holds the binary and media assets shipped in the source tree,
plus the project-authored data files that configure them. Every
binary/media asset is mirrored in [`third_party/assets.json`](../third_party/assets.json),
which carries SHA-256 hashes and full provenance metadata;
[`../THIRD_PARTY_LICENSES.md`](../THIRD_PARTY_LICENSES.md) summarizes the legal
terms, and [`third_party/licenses/`](../third_party/licenses/) stores the
license texts and rights-holder notices. Run
`bash scripts/check-asset-provenance.sh` after replacing any binary asset:
the check recomputes every hash against the manifest.

## Third-party assets

### `fonts/NotoSansJP-Regular.ttf`

- **Source**: [Noto Sans JP](https://github.com/google/fonts/tree/main/ofl/notosansjp)
  (Google Fonts / Noto Project authors)
- **License**: [SIL Open Font License 1.1](https://github.com/google/fonts/raw/main/ofl/notosansjp/OFL.txt);
  full text at [`third_party/licenses/OFL-1.1-NotoSansJP.txt`](../third_party/licenses/OFL-1.1-NotoSansJP.txt).
- **Modification status**: no repository-local modifications. The exact upstream
  release is not hash-matched; the font family is identified but the checked-in
  file has not been pinned to a specific upstream revision.

### `characters/Alicia/AliciaSolid.vrm`

- **Source**: [Alicia Solid](https://3d.nicovideo.jp/alicia/) by DWANGO Co., Ltd.
  (Niconi 3D-chan series)
- **License**: VRM Platform License 1.0 plus the
  [Niconi 3D-chan license](https://3d.nicovideo.jp/alicia/rule.html); see
  [`third_party/licenses/alicia.md`](../third_party/licenses/alicia.md). The
  embedded VRM metadata declares `allowRedistribution: false`,
  `modification: prohibited`, `commercialUsage: personalProfit`, and
  `creditNotation: required`.
- **Modification status**: unmodified original download.
- **Distribution status**: development fixture only. Because the embedded
  metadata disallows redistribution and modification, this file must not be
  copied into a release artifact or redistributed as a standalone asset.

### `characters/Alicia/motions/VRMA_01.vrma` – `VRMA_07.vrma`

- **Source**: [VRM Animation 7-piece set](https://booth.pm/ja/items/5512385)
  published by pixiv Inc.'s VRoid Project
- **License**: VRoid Project VRM Animation terms of use; see
  [`third_party/licenses/vroid-vrma.md`](../third_party/licenses/vroid-vrma.md).
  Modification and commercial use are permitted under the published terms with
  this attribution:

  > Character animation credits to pixiv Inc.'s VRoid Project

  Distributing the motion or an altered work in an extractable-file state is
  prohibited without permission.
- **Modification status**: unmodified originals from the distributed set.
- **Distribution status**: development fixture only; do not copy into a release
  artifact or redistribute as standalone assets.

## Project-owned assets

| Path | Purpose | License |
| --- | --- | --- |
| `icon.png` | Application icon | Root [`LICENSE`](../LICENSE) (MIT), owned by ene contributors |
| `characters/Alicia/character.json` | Default character card (Character Card V3) | Root LICENSE, authored for Ene |
| `characters/Alicia/character_settings.json` | Per-character stage settings | Root LICENSE, authored for Ene |
| `lang/en/patterns.json`, `lang/ja/patterns.json` | Language pattern tables | Root LICENSE, authored for Ene |
| `settings.json` | Default application settings seed | Root LICENSE, authored for Ene |

These files are original project content, so they inherit the repository
license rather than requiring separate attribution.
