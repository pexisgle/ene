# Stage UI 検証: Slint + 共有 wgpu

将来の Stage 窓で、3D（wgpu / `ene-vrm`）と 2D UI（Slint）を
1 枚の透明 native window に載せるための技術検証です。
**製品経路ではありません。** 本番 `ene-stage` は egui chrome + wgpu
オーバーレイのままです。

コード: `crates/ene-stage-poc/`

| バイナリ | 実験 |
|---|---|
| `ene-stage-poc-a` | A: 共有 wgpu 合成 |
| `ene-stage-poc-b` | B: UI → VRM → passthrough ルーティング |
| `ene-stage-poc-baseline` | 同じ `GpuContext` 上の egui + 三角形 |

```sh
ENE_STAGE_POC_SECONDS=8 cargo run -p ene-stage-poc --bin ene-stage-poc-a
ENE_STAGE_POC_SECONDS=8 cargo run -p ene-stage-poc --bin ene-stage-poc-b
ENE_STAGE_POC_SECONDS=8 cargo run -p ene-stage-poc --bin ene-stage-poc-baseline
```

`Esc` または `q` で終了。`ENE_STAGE_POC_SECONDS` は N 秒後に自動終了し、
idle / animation のサンプルを残します。

---

## 実験 A — 共有 wgpu + Slint 合成

### 実装した方式

プローブが winit 窓と wgpu surface を**所有**します。本番オーバーレイ
（`apps/ene-stage/src/overlay.rs`）と同じ持ち方です。

1. `ene-stage` と同じバックエンド規則で `GpuContext` を作る
   （Linux は PRIMARY / Vulkan、Windows は DX12 + `DxgiFromVisual`）。
2. 透明・枠なし・常に最前面の窓を 1 枚作る。
3. カスタム `WindowAdapter` を `slint::platform::set_platform` に渡し、
   レンダラは `FemtoVGWGPURenderer::new(instance, device, queue)`。
   引数は `GpuContext` の**同じ** wgpu オブジェクトの clone。
4. 毎フレーム、GPU のみ:

   wgpu 3D（三角形または `ene-vrm`）→ 透明クリア
   → Slint FemtoVG をオフスクリーン `Rgba8Unorm` へ
   → GPU blit（premultiplied）で surface へ
   → `present`

Slint は第二の swapchain を持ちません。
`FemtoVGWGPURenderer::render_to_texture_view` は、共有デバイス上で
こちらが確保したテクスチャへ描きます。

VRM: 実ファイルなら `assets/characters/Alicia/AliciaSolid.vrm` を
`ene-vrm` で読みます（1 KiB 超の GLB）。なければ
`ene_vrm::minimal` を一時ファイルへ書きます。描画はオーバーレイと同じ
`VrmRenderer::render`（command encoder + depth、CPU readback なし）。
VRM が使えなくても三角形でパス順は証明できます。後から同じ
encoder / surface view に差し込めます。

### Device/Queue 共有

**成立。** `GpuHandles` が `GpuContext` の `Instance` / `Device` /
`Queue` を clone し、`FemtoVGWGPURenderer::new` に渡します。
wgpu 29 の `Device`/`Queue` は同じ GPU オブジェクトの安価な clone です。
三角形 / VRM のパイプラインは `GpuContext.device` 上、Slint FemtoVG は
その clone を使います。

公式の `BackendSelector::require_wgpu_29(WGPUConfiguration::Manual)` は
メイン経路には使いませんでした。あちらは Slint が窓と surface を所有します。
「Slint が独自 surface を要求するか」への答えは、カスタム platform +
FemtoVG WGPU なら **要求しない** です。

`unstable-wgpu-29` は不安定 feature です。Slint が wgpu 30 に上がれば
`unstable-wgpu-30` への追従が必要です。実行時 blocker ではなく保守コストです。

### ゼロコピー合成

**3D + UI 経路では成立。** 合成経路に `copy_texture_to_buffer`、
`map_async`、`SharedPixelBuffer` はありません（ソース走査の unit test）。
Slint は GPU テクスチャへ書き、こちらが blit します。blit は UI 層の
GPU コピーであり、GPU→CPU→GPU ではありません。

FemtoVG の WGPU レンダラはターゲットをクリアするので、UI を 3D と同じ
pass には置けません。オフスクリーン UI + blit で
**3D → UI → present** を守ります。

### 透明ウィンドウ

`ene-stage` と同じ `pick_alpha_mode` /
`CompositeAlphaMode::{Pre,Post}Multiplied` です。lavapipe は
`PreMultiplied` を出すことが多いです。Windows は
`with_no_redirection_bitmap(true)` + DX12 visual swapchain が必要で、
`gpu.rs` から踏襲しています。

### 性能

Cloud Agent Linux（`DISPLAY=:1`、lavapipe、debug ビルド）で
`ENE_STAGE_POC_SECONDS=8` を走らせた値です。

Adapter: `llvmpipe (LLVM 20.1.2, 256 bits)`、Vulkan。
`transparency=true`（`Bgra8UnormSrgb` / `PreMultiplied`）。
VRM は `ene_vrm::minimal` フィクスチャ（この workspace に Alicia なし）。
VRAM は lavapipe から取れないので RSS を代理にします。`max_ms` は
初回シェーダコンパイルのヒッチです。

| プローブ | フェーズ | frames | 平均 | 最大 | CPU user | CPU sys | RSS 開始 | RSS 終了 |
|---|---|---|---|---|---|---|---|---|
| A Slint+VRM | idle 3s | 2 | 135.1 ms | 169.5 ms | 280 ms | 20 ms | 122 MiB | 167 MiB |
| A Slint+VRM | animation 5s | 657 | 11.77 ms | 2740 ms | 10480 ms | 610 ms | 167 MiB | 169 MiB |
| B input | idle 8s | 2 | 133.6 ms | 162.4 ms | 260 ms | 50 ms | 122 MiB | 167 MiB |
| egui+三角形 | idle 3s | 1 | 21.1 ms | 21.1 ms | 20 ms | 0 | 114 MiB | 137 MiB |
| egui+三角形 | animation 5s | 2958 | 2.70 ms | 2990 ms | 6020 ms | 1570 ms | 137 MiB | 141 MiB |

Idle は本物の `WaitUntil`（数回 present して停止）。Animation は `Poll`。
Slint+VRM が egui 三角形より遅いのは VRM + オフスクリーン UI + blit を
描いているからで、FemtoVG が本質的に 4 倍遅いからではありません。
lavapipe では animation の CPU が wall を超えます（ソフトウェアラスタ）。
製品 GPU の数字ではありません。

Device/Queue 共有: **成立**（ログ
`FemtoVGWGPURenderer constructed from cloned GpuContext instance/device/queue`）。
GPU→CPU コピー: 合成経路には**ない**。

### 問題点

- `FemtoVGWGPURenderer` + カスタム platform は **不安定 API**。
- FemtoVG はターゲットをクリアするので、blit か `Image` underlay が必要。
- FemtoVG の `set_rendering_notifier` underlay には、毎フレーム
  パイプライン再生成の既知バグがある。このプローブはその API を使わない。
- `Button` は `std-widgets.slint` の import が必要。`#8ecbff88` のような
  16 進は `8e` が指数としてパースされる。
- このツリーの wgpu 29 は `multiview_mask`、
  `depth_write_enabled: Some(…)`、`bind_group_layouts: &[Some(…)]`。
  古い wgpu の Slint 例はそのままでは動かない。

### Slint 採用の blocker か

**合成そのものの実行時 blocker はない。** カスタム platform を
受け入れるなら進める。`unstable-wgpu-29` の保守リスクはあるが、
ene は既に wgpu 29 をピンしている。

---

## 実験 B — 入力ルーティング / VRM hit-test / passthrough

### ルーティング

`crates/ene-stage-poc/src/input.rs` の純関数:

```
Pointer → UI 矩形 → VRM 粗パーツ → passthrough
```

重なりは UI 優先。クリック時:

```
UI hit: …  VRM hit: …  passthrough: …  target: …
```

優先度・手と胴の重なり・空領域・OS input region の和は、
GUI なしの unit test でカバーしています。

### VRM hit-test

CPU 側の画面空間矩形: **head / torso / left hand / right hand**。
VRM があるときは正規化 AABB（オーバーレイと同じ auto-fit）から作り、
なければ三角形を覆うプレースホルダです。

**GPU picking は実装していません。** 理由:

- 必要なのは「部位 / 空のデスクトップ」であり、三角形単位ではない。
- GPU picking は ID バッファと readback が要る。カーソル付近の小タイルでも
  GPU→CPU 同期になり、毎フレームや静止中の継続 readback はこの検証が
  禁じるコストモデルです。
- stage は既に AABB レイ（`drag::hit_test`）でシルエット操作できている。
- 将来の任意経路: CPU 粗判定 miss なら何もしない。重なり部位だけ
  小タイル GPU pick。入力モデルの証明には不要。

### OS レベルの click-through

`StageInputRegion`（`set_passthrough` / `update_input_region`）。

アプリ内ルーティングと混同しないこと。コンポジタは OS input region の
内側にしかイベントを渡しません。region が空なら ene はクリックを
見ません。それが passthrough です。

| OS | 窓全体 click-through | インタラクティブ部分だけ | 制約 |
|---|---|---|---|
| **Windows** | 可。`set_cursor_hittest(false)` + `WS_EX_TRANSPARENT`。 | 可。UI+VRM 矩形の和を `SetWindowRgn`（ピクセル単位のアルファではない）。`WM_NCHITTEST` / DComp は後段。 | 本番オーバーレイの HWND 経路と同じ。 |
| **X11** | 可。XShape `Kind::Input` の空リスト。プロトコルは**動く**: SHAPE 1.1 があり、`shape::rectangles(SET, INPUT)` は成功し、同一接続の `get_rectangles` は UI∪VRM 集合を返す。 | 可。矩形の和。 | **コンポジタの制約（xfwm4 で実測）:** WM が reparent したあと、およそ 1 フレームで Input を窓全体へ戻す。外部 `XShapeGetRectangles` は `0,0 800×600` になり、透明部分のクリックもクライアントに届く。`_NET_WM_BYPASS_COMPOSITOR=1` では止まらなかった。プロセス内ルーティングはそれを `Passthrough` と分類する。本番 stage の `set_cursor_hittest` は Linux で no-op。desktop は既に shape を使う。**グローバルポインタを `query_pointer` できる。** WM ごとに要確認（openbox / コンポジタなし / picom）。 |
| **Wayland** | 可。空の `wl_surface::set_input_region`。 | 可。矩形の和。**ピクセル単位ではない。** | **グローバルポインタ照会がない。** region が空だとクライアントはポインタイベントを**一切**受け取らず、ホバーで穴を開けられない。成立する設計は常に OS region = UI ∪ VRM 粗矩形。`zwlr_layer_shell_v1` は全画面の上に載せる任意機能で、input region には不要。winit 0.30 の `set_cursor_hittest` は **Linux では no-op**（`ene-desktop` / `ene-stage` の platform モジュールに記載済み）。 |

**Wayland は入力モデルの blocker ではない**が、
「全面 click-through + ホバーで穴」は **blocker** です。
ene-stage は既に「Wayland では click-through を切ってからドラッグ」と
書いています。PoC の `update_input_region(rects)` がその穴を閉じる設計です。

この Cloud Agent は `DISPLAY=:1`（X11、xfwm4）です。Wayland コードは
コンパイルされ、プロトコルは `ene-desktop` の `wayland_region.rs` と同じです。
コンポジタ上では未走行です。

### クリックログ（実験 B、実ポインタ）

`xdotool mousemove` のあと別プロセスで `click`（`click --window` は
XSendEvent になり hit-test を飛ばす）:

```
UI hit: true   VRM hit: false  passthrough: false  target: Ui
UI hit: false  VRM hit: true   passthrough: false  target: Vrm(Torso)
UI hit: true   VRM hit: false  passthrough: false  target: Ui   # overlap
UI hit: false  VRM hit: false  passthrough: true   target: Passthrough
```

この xfwm4 セッションでは透明部分のイベントもクライアントに届きました
（上のコンポジタ制約）。プロセス内ルータは UI/VRM としては扱いません。

### 性能上の懸念

ルーティングはポインタイベントあたり数個の AABB。本番では
OS region 更新をスロットルすべきです（desktop は `about_to_wait`）。
GPU readback なし。

### blocker か

**ない。** OS 差は `StageInputRegion` に閉じる。共通化して忘れない。

---

## 測定（Cloud Agent Linux）

実験 A の性能表を参照。バイナリが印字した生データ:

```
=== experiment-a ===
adapter: llvmpipe (LLVM 20.1.2, 256 bits)
backend: Vulkan
shared_device=true transparency=true vrm=true input=x11 partial_region=true zero_copy=gpu-texture-blit
phase=idle wall_ms=3000.5 frames=2 avg_ms=135.10 max_ms=169.51 cpu_user_ms=280.0 cpu_sys_ms=20.0 rss_start_kib=125168 rss_end_kib=170852
phase=animation wall_ms=5005.6 frames=657 avg_ms=11.77 max_ms=2740.44 cpu_user_ms=10480.0 cpu_sys_ms=610.0 rss_start_kib=170852 rss_end_kib=173092

=== experiment-b ===
phase=idle wall_ms=8000.7 frames=2 avg_ms=133.63 max_ms=162.39 cpu_user_ms=260.0 cpu_sys_ms=50.0 rss_start_kib=124828 rss_end_kib=170832

=== egui-baseline ===
phase=idle wall_ms=3000.3 frames=1 avg_ms=21.13 max_ms=21.13 cpu_user_ms=20.0 cpu_sys_ms=0.0 rss_start_kib=116288 rss_end_kib=140176
phase=animation wall_ms=5000.9 frames=2958 avg_ms=2.70 max_ms=2989.63 cpu_user_ms=6020.0 cpu_sys_ms=1570.0 rss_start_kib=140176 rss_end_kib=144556
```

VRAM: lavapipe では取得不可。`nvidia-smi` なし。
GPU→CPU コピー: 合成経路にはない。

---

## 手動確認（unit test にしない）

コンポジタの挙動は unit test しません。実デスクトップでは:

1. `ene-stage-poc-a`。透明窓、3D（または三角形）の上に角丸吹き出し、
   ボタン、resize、DPI（モニタ跨ぎ）。
2. `ene-stage-poc-b`。吹き出し → `target: Ui`。キャラ → `target: Vrm(…)`。
   透明部分 → 背後のアプリへ。正しい input region なら空クリックは
   クライアントに届かない。
3. 吹き出しと胴が重なる → UI 優先。
4. Windows: region 空で `WS_EX_TRANSPARENT`。
5. Wayland: Sway/KWin で `set_input_region`。空 region へのホバーは期待しない。

---

## 最終判定

**B. 一部 architecture 変更が必要だが進めてよい。**

**A ではない理由:**

- Slint は FemtoVG のクリアのため、blit か `Image` underlay が要る。
- Wayland は「全面 click-through + ホバー」ができない。
- `unstable-wgpu-29` とカスタム `Platform` は保守面。

**C ではない理由:**

- Device/Queue 共有は Slint のサポート API。
- GPU ゼロコピー合成は動く。
- 透明は既存オーバーレイと同じ。
- 入力は純関数 + 小さな OS 抽象で、desktop が X11/Wayland で既に実証。

| 観点 | 結果 |
|---|---|
| 性能 | 実 GPU なら UI blit は安い。lavapipe の animation は CPU 律速。readback なし。 |
| メモリ | Idle RSS は Slint+VRM 約 167 MiB、egui+三角形 約 137 MiB。窓サイズの Rgba8 UI ターゲットが約 2 MiB。 |
| 入力 | 純関数テストは通る。この xfwm4 では透明クリックもクライアントに届き、ルータは `Passthrough` とラベルした。Wayland の input region が移植可能な OS モデル。 |
| 透明 | stage と同じ alpha mode 選択。 |
| 移植 | Windows / X11 / Wayland いずれも可能。API は違う。 |
| 保守 | 不安定な Slint wgpu feature。wgpu と一緒に bump。 |

---

## この検証でやらなかったこと

- chat / detail / theme の Slint 移植なし。
- egui 削除なし。
- 本番オーバーレイの変更なし。
- GPU picking なし。
- この Cloud Agent VM での Wayland コンポジタ走行なし（`DISPLAY=:1` は X11）。
