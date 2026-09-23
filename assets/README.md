# Assets

The desktop Body implementation reads two kinds of asset: the sample model
committed to this repository, and the motion pack installed locally because it
must not be redistributed.

## `seed-san.vrm` (bundled)

`seed-san.vrm` is an unmodified copy of the VRM 1.0 sample model **Seed-san**.
It is a development sample, not the official `ene` character. It provides the
humanoid, expression, LookAt, MToon, node-constraint, and SpringBone data
needed by the desktop Body implementation.

- Creator and copyright holder: VirtualCast, Inc.
- Model name: Seed-san
- License: [VRM Public License 1.0](https://vrm.dev/licenses/1.0/)
- Source: [`vrm-c/vrm-specification` at commit `837f156d`](https://github.com/vrm-c/vrm-specification/blob/837f156dbce43ad69183ce1bdab549961ae1c1ee/samples/Seed-san/vrm/Seed-san.vrm)
- SHA-256: `624d0d554bc205bbdc33e22a68a2c3c20edebb3e573011ead8878a65e5329b23`

The model's embedded license settings require credit, permit redistribution,
permit avatar use by everyone, permit corporate commercial use, and permit
modification and redistribution. The model is distributed under its embedded
VRM Public License terms, not the repository's MIT license. Its inclusion does
not imply that VirtualCast, Inc. endorses ene.

## VRoid motion pack (install asset)

The desktop Body plays the VRoid `VRMA_MotionPack` (7 clips,
`VRMC_vrm_animation` 1.0) for the activity hints. The pack is an **install
asset**: it is not in this repository, and its terms forbid distributing the
motions, or their modified versions, in a form they can be extracted from.

- Distributor: pixiv Inc. (VRoid Project)
- Distribution page: [VRMアニメーション7種セット（.vrma）](https://booth.pm/ja/items/5512385) (free BOOTH download)
- Archive: `VRMA_MotionPack.zip`, holding `vrma/VRMA_01.vrma` … `VRMA_07.vrma`
- Terms: copyright belongs to pixiv Inc. regardless of modification;
  modification and commercial use are allowed, and commercial use requires the
  credit 「キャラクターアニメーション: ピクシブ株式会社 VRoidプロジェクト」or
  "Character animation credits to pixiv Inc.'s VRoid Project"; distributing the
  motions, or their modified versions, in extractable form is prohibited.

Download the archive from the distribution page, extract the seven
`VRMA_*.vrma` files, and place them in `assets/motions/` (this directory, not
committed) for a development run. The application only reads them from its
search order at runtime: nothing is downloaded or copied. The search order, the
pose → clip assignment, and the behavior without the pack (`HealthTick.motion:
Unsupported`) are documented in
[`apps/ene-body/README.md`](../apps/ene-body/README.md#motion-pack-vrma), along
with the asset probe that verifies a placed pack.
