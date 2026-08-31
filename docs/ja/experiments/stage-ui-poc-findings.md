# Stage UI PoC の検証結果

この文書は、draft PR #1258 で得られたアーキテクチャ上の検証結果を、単発実験用の `ene-stage-poc` 実装そのものを main workspace に取り込まずに保存するためのものです。

元 snapshot: #1258 の `58421613f413df858c2ec389e4772cf1ee665e08`。

## 対象

PoC では、既存の VRM/wgpu renderer と Slint UI を同一の透明 native window 上で合成する経路と、platform ごとの input region 挙動を検証しました。production 実装ではなく technical spike です。

## 採用する描画上の知見

- Slint は VRM renderer と同じ wgpu 29 の `Instance` / `Device` / `Queue` を共有できる。
- 検証できた composition path は GPU 内で完結する。
  1. VRM/3D を描画する。
  2. Slint FemtoVG を offscreen `Rgba8Unorm` target へ描画する。
  3. `LoadOp::Load` と premultiplied-alpha を使う fullscreen pass で UI target を合成する。
  4. 既存 Stage surface を present する。
- GPU→CPU readback は不要で、`copy_texture_to_texture` を前提にしない。
- Slint FemtoVG は target を clear するため、既存の 3D pass に直接重ねるのではなく、offscreen UI target + compositor pass が必要。
- `unstable-wgpu-29` は upgrade/maintenance risk だが、runtime blocker は確認されていない。Stage renderer 境界の内側へ隔離する。

## Performance evidence

Experiment C では Linux Vulkan / llvmpipe 上で release build の相対比較を warm-up 後に行いました。実 GPU の acceptance benchmark ではなく、経路ごとの追加コストを見るための数値です。

| 差分 / 経路 | 結果 |
|---|---:|
| Slint init + offscreen target + premultiplied compositor (`C1 - C0`) | 平均 frame +2.79 ms |
| static Slint bubble (`C2 - C1`) | +0.68 ms |
| light Slint animation (`C3 - C2`) | +0.05 ms |
| Slint offscreen path (`C2`) | 平均 frame 5.44 ms |
| egui Load-on-swapchain 比較 (`C4`) | 平均 frame 2.08 ms |

変更がない idle 状態では continuous frame が 0、CPU もほぼ 0 でした。PoC 上では Slint の redraw scheduling は blocker ではありません。ただし production acceptance では実 GPU / release build の再計測が必要です。

## process 内 interaction model

検証した routing priority は次の通りです。

```text
UI -> VRM -> passthrough
```

UI と VRM の interaction geometry が重なった場合は UI を優先します。VRM interaction は coarse な CPU geometry で実装可能で、常時 GPU picking/readback が必要という根拠は得られていません。

`VisualGeometry` と `InteractionGeometry` は別概念として扱い、platform API はその projection だけを受け取ります。Slint component に platform policy を漏らしません。

## Wayland の知見

native Weston 13 の検証では `wl_surface::set_input_region` が期待通り動作しました。

- UI / VRM region 内は pointer input を受け取る。
- empty glass は Stage surface に届かず、背後 surface へ通せる。
- OS region を現在の interactive geometry の union として維持すれば hover-rearm は不要。
- 毎 frame region を更新する必要はなく、geometry 移動中のみ dirty tracking + throttle で十分。

これは Weston 13 での確認結果であり、すべての Wayland compositor への保証ではありません。production では compositor-aware な diagnostics / fallback policy を維持します。

## X11 の知見

xfwm4 の検証から、SHAPE の挙動は WM 依存であることが分かりました。

- Input-only SHAPE は、検証した xfwm4 環境で短時間後に full client region へ戻された。
- 単純形状では Bounding と Input を同時設定すると維持できる場合がある。
- 単純な `Bounding > Input` では Input 内を Stage が受け取り、Input 外を別 process の click sink へ通せた。
- 複雑な Bounding union は WM により full client へ拡張される場合がある一方、Input は小さいまま / YX-banded で残る場合がある。
- Bounding 外の pixel は clip されるため、effect 用の coarse Bounding には padding が必要。
- `ShapeNotify` ごとの再設定は約 151 updates/s で WM と競合したため明確に不採用。

したがって production X11 は pixel-perfect region や continuous reapply ではなく、coarse SHAPE + fallback を採用します。

## Windows の知見と棄却した根拠

元の Experiment B では partial-region probe として `SetWindowRgn` を使っていました。しかし、この結果を input-only partial click-through の成立根拠として扱ってはいけません。`SetWindowRgn` は window の shape / visible region 自体を変更するため、Stage 全体を描画したまま一部だけ cross-process input を受けることを証明していません。

そのため Windows の採用方針は次の通りです。

- 既存の DX12 / DirectComposition / wgpu Stage window architecture を維持する。
- `StageInteractionController` の下で window-wide cursor hit testing（`set_cursor_hittest()` 相当）を維持する。
- `Passive` / `Interactive` / `Dragging` / `UiFocused` を明示的な lifecycle state とする。
- Stage v2 では layered/helper/small-window や partial cross-process click-through を追求しない。

#1258 の raw branch に残る Windows partial-region の肯定的な記述は、この結論および #1260/#1268 の Stage v2 方針によって superseded されています。

## Stage v2 に引き継ぐアーキテクチャ判断

PoC の結果から、production では以下を採用します。

- shared-wgpu による VRM + Slint composition は成立する。
- UI composition は premultiplied alpha を使い GPU 内で完結させる。
- idle 時は continuous redraw しない。
- hit-test priority は UI > VRM。
- `VisualGeometry` と `InteractionGeometry` を分離する。
- Wayland は interaction geometry を input region へ projection する。
- X11 は coarse Bounding/Input SHAPE + fallback とし、WM と競合しない。
- Windows は現在の DComp/wgpu window と window-wide interaction mode を維持する。
- platform policy を Slint component code に埋め込まない。

## 意図的に main へ入れないもの

#1258 の `crates/ene-stage-poc` は production workspace に取り込みません。将来 regression / diagnostic test として価値が出た probe のみ、maintained purpose と acceptance criterion を持たせて個別に再導入します。

将来的に残す価値が高い候補は次の通りです。

- shared-wgpu compositor benchmark / sanity probe
- Wayland input-region sanity probe
- X11 SHAPE + click-sink sanity probe
- Windows current-architecture interaction regression probe

production 実装と最終 cross-platform acceptance は #1260 とその子 issue で追跡します。
