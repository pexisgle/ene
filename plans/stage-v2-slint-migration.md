# Stage v2 / Slint Migration Plan

> Tracking epic: #1260  
> Technical probes: #1258 and `docs/experiments/stage-ui-*.md` / `docs/ja/experiments/stage-ui-*.md`

## Goal

`ene-stage` の egui UI を段階的に Slint へ移行し、同時に Stage の VRM rendering・2D UI・input を明確な責務境界へ再設計する。

これは単なる egui → Slint の置換ではない。最終的に次の責務を分離する。

```text
App / Core
   |
StageState / Actions
   |
StageScene
   +-- VisualGeometry
   +-- InteractionGeometry
   +-- StageInteractionController
   |
StageRenderer
   +-- VRM / wgpu
   +-- Slint offscreen renderer
   +-- premultiplied-alpha compositor
   |
StagePlatform
   +-- Wayland: input region
   +-- X11: SHAPE + fallback
   +-- Windows: current DComp/wgpu window + window-wide hittest
```

## Decisions fixed by the experiments

### Rendering

- Slint + shared wgpu 29 は成立する。
- VRM と Slint は同一 `Instance` / `Device` / `Queue` を共有する。
- Slint は offscreen `Rgba8Unorm` target に描画する。
- VRM の上へ fullscreen triangle の premultiplied-alpha pass で合成する。
- CPU readback は行わない。
- idle 時は continuous redraw しない。
- `unstable-wgpu-29` 依存は renderer integration の狭い境界へ隔離する。

### Windows

- 現行 DX12 / DirectComposition / wgpu StageWindow を維持する。
- window-wide `Window::set_cursor_hittest()` を維持する。
- `Passive / Interactive / Dragging / UiFocused` を明示した controller を先に導入する。
- passive 中に Stage は pointer event を受けないため、clickable UI は表示側が interactive entry を要求する。
- layered full-frame presentation、helper HWND、small/tight-window architecture への全面移行は行わない。
- cross-process partial click-through を再追求しない。

### Wayland

- `InteractionGeometry` を `wl_surface::set_input_region` へ反映する。
- hover-rearm は使わない。
- dirty + 数px threshold + rate limit を使い、static 時は更新しない。
- Weston 13 で成立したが Wayland 全compositorでの保証とはしない。

### X11

- SHAPE を coarse filter として使う。
- `VisualGeometry` は少数rect/AABB + effect padding の Bounding へ落とす。
- `InteractionGeometry` は coarse Input へ落とす。
- complex Bounding union がそのまま維持されることを前提にしない。
- Input の YX-banding を前提にする。
- ShapeNotify reapply で WM と競合しない。
- runtime sanity / fallback を持つ。

### VRM interaction

- window-level input と body-part hit-test は別問題として扱う。
- 第一候補は bone-derived CPU collider / coarse screen-space regions。
- GPU picking は CPU方式で実用上不足するケースが確認された場合のみ追加する。

### UI migration

- Stage を最初の vertical slice とする。
- Stage が production gate を通るまで Chat / Detail の本格移行を開始しない。
- Chat は既存 `ChatMessageView` / `SurfaceAction` を利用する。
- Detail は一括 rewrite せず、state/action/rendering を先に分離してタブ群ごとに移行する。
- user theme は Slint code ではなく data-only token format とする。

## Non-goals

- Windows partial click-through のための Win32 trick の追加調査
- X11 pixel-perfect input/visual silhouette
- 全UIの一括 rewrite
- Detail を最初に移植すること
- Stage v2 と同時に core/session/provider semantics を変更すること
- GPU readback を前提にした presentation

---

## Phase 0 — Baseline

### #1261 `test(stage-v2): 移行前 baseline と既存 flaky test を固定する`

後続PRで regression を判定する基準を固定する。

Gate:
- 既存 flaky test の test 名 / failure rate を記録
- Windows/Linux startup, idle CPU, RSS, redraw behavior を記録
- PoC結果を regression checklist 化

---

## Phase 1 — Interaction lifecycle

### #1262 `refactor(stage): StageInteractionController で Windows interaction lifecycle を一元化する`

Slintを入れる前に入力状態を安定させる。

最低状態:

```text
Passive
Interactive
Dragging
UiFocused
```

必須 cancel path:
- focus lost
- pointer release/capture lost
- avatar reload/remove/no-avatar
- window hide/shutdown
- UI focus release

Gate:
- `set_cursor_hittest()` の変更authorityを一元化
- drag/hover/focusのstale stateを残さない
- Windowsの現行UXを変更しない

---

## Phase 2 — Stage model

### #1263 `refactor(stage-v2): StageScene と Visual/InteractionGeometry を導入する`

UI toolkit / OS windowingから独立したscene modelを作る。

重要な不変条件:

```text
VisualGeometry != InteractionGeometry
```

Visual-only glow/shadow/particle と interactive UI/VRM collider を別々に表現する。

---

## Phase 3 — Renderer

### #1264 `refactor(stage-v2): OverlayWindow から StageRenderer を分離する`

`OverlayWindow` からGPU rendering責務を分離する。

### #1265 `feat(stage-v2): Slint shared-wgpu renderer と premultiplied compositor を production 化する`

PoCのGPU-resident compositionをproduction化する。

Performance gate:
- release + 実GPU
- idle CPU ≈ 0
- static/animated Stage UI が60fpsを安定して超える
- p95/p99 と RSS を #1261 baseline と比較

ここで renderer integration が成立しない場合はUI移植を進めない。

---

## Phase 4 — Platform input backends

### #1266 Wayland

`InteractionGeometry` → `wl_surface::set_input_region`。

### #1267 X11

coarse Bounding/Input + fallback。ShapeNotify fightは禁止。

### #1268 Windows

現行 DComp/wgpu StageWindow + window-wide cursor hittest を controller に接続する。

Platform差は `StagePlatform` 境界で止め、Slint component / VRM renderer へ漏らさない。

---

## Phase 5 — Minimal Slint Stage

### #1269 `feat(stage-v2): 最小 Slint Stage shell を production に導入する`

最初は bubble/text/button のみ。

Gate:
- VRM + Slint + compositor + platform input がproduction codeで統合される
- display-only UI は passive を維持できる
- clickable UI は最初のclickから操作できる
- idle redrawが止まる

---

## Phase 6 — Stage interaction features

### #1270 VRM → UI anchors

head/face/chest/hands等のboneをscreen-space UI anchorへ投影する。

### #1271 VRM hit-test v2

CPU colliderを第一候補として hover/click/drag を同一sourceへ統一する。

---

## Phase 7 — Design system

### #1272 `feat(stage-ui): Slint design system と data-only user theme を導入する`

Tokens:
- colors
- typography
- spacing
- radius
- shadow
- motion

初期components:
- EneButton / EneIconButton
- EneCard
- SpeechBubble / StatusBubble
- Chip/Badge
- basic input wrapper

User themeはTOML等のdata-only formatでvalidation/fallbackを持つ。

---

## Phase 8 — Production Stage gate

### #1273 `feat(stage-v2): Stage UI を Slint production path へ切り替える`

Stageを最初のproduction vertical sliceとして完成させる。

必須regression:
- transparency
- always-on-top
- platform input behavior
- VRM hover/click/drag
- anchor follow
- DPI / resize
- multiple avatar
- idle CPU / RSS / frame time

**#1273 が安定するまでは Chat / Detail の本格移行を開始しない。**

---

## Phase 9 — Small surfaces

### #1274 Chat

既存 `ChatMessageView` / `SurfaceAction` を利用してSlint化する。

### #1275 Spotlight / Caption / Approval

小規模surfaceをまとめて移行する。CaptionをStageへ統合するかは責務/UXで判断し、統合自体を目的にしない。

---

## Phase 10 — Detail preparation

### #1276 `refactor(stage-detail): DetailUiState / actions / rendering を分離して移行可能にする`

巨大な `detail/mod.rs` を一括rewriteしない。

目標は state/action/view-data と egui rendering のcouplingを切ることだけで、過剰なMVVM化はしない。

---

## Phase 11 — Detail migration

### #1277 Home / Companion

最初のsettings vertical slice。

### #1278 Conversation / Voice

large model list / voice controls / form focusの移行。

### #1279 Memory / Work

long list/table、job state、progress/actionの移行。

### #1280 Connections / System / Log

settings/schema/diagnostics/log系を移行し、Detailのproduction egui rendererをなくす。

Detail移行中は Stage/Chat=Slint、残存Detail=egui の混在を許容する。

---

## Phase 12 — Acceptance gate

### #1281 `test(stage-v2): cross-platform acceptance / performance regression gate を通す`

Windows / Wayland / X11 で全production surfaceをsmokeし、#1261 baselineと比較する。

Blockerを cleanup issueへ持ち越さない。必要なら別issue化してgateを止める。

---

## Phase 13 — Remove egui

### #1282 `chore(stage-ui): egui依存を削除し Stage v2 PoC/診断コードを整理する`

- `egui`
- `egui-wgpu`
- `egui-winit`
- `chrome.rs` 等の残存renderer/primitives
- dual-render / temporary bridge

を削除する。

PoCは全削除せず、platform regression/diagnosticsとして価値があるものを残す。

候補:
- shared-wgpu compositor benchmark
- Wayland input region sanity
- X11 SHAPE/click-sink sanity
- Windows current-architecture regression probe

---

## Dependency overview

```text
#1261 baseline
  |
#1262 interaction controller
  |
#1263 StageScene / geometry
  |
#1264 renderer split
  |
#1265 Slint compositor
  +----------------------+----------------------+
  |                      |                      |
#1266 Wayland         #1267 X11             #1268 Windows
  +----------------------+----------------------+
                         |
                      #1269 minimal Slint Stage
                       /    \
                  #1270    #1271
                       \    /
                       #1272 design system
                         |
                       #1273 production Stage gate
                      /     |       \
                 #1274   #1275    #1276
                                  / |  | \
                              #1277 #1278 #1279 #1280
                                  \  |  |  /
                                    #1281
                                      |
                                    #1282
```

## Merge / PR strategy

- 1 issue = 原則1レビュー可能単位。
- 大きいissueは機能flag/dual pathを使ってmainを常にbuildable/runnableに保つ。
- 長寿命の全面rewrite branchを作らない。
- stack PRが必要な場合も、基盤→利用側の順で親からmergeできる形にする。
- migration中に見つけたunrelated bugはこのscopeへ混ぜず、既存issueへ参照または別issue化する。

## Completion definition

#1260 は以下をすべて満たしたときだけcloseする。

1. #1281 acceptance gateが通っている。
2. production surfaceがSlint pathを使用する。
3. productionからegui dependenciesが削除されている。
4. Windowsは現行DComp/wgpu + controller方式を維持している。
5. Wayland/X11のinput backendとfallbackが文書化されている。
6. VRM hit-testとUI anchorがStage v2境界内で動く。
7. user theme/design systemがhard-coded stylingの代替として機能する。
8. PoC/ADRから技術判断を追跡できる。
