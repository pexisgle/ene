# Stage UI 検証 2: 公平な compositor コストと Linux 入力領域

[Stage UI 検証](stage-ui-poc.md) の続きです。**製品経路ではありません。**
本番 `ene-stage` は変更していません。

コード: `crates/ene-stage-poc/`

| バイナリ | 実験 |
|---|---|
| `ene-stage-poc-c` | C: 公平な compositor コスト（C0–C4） |
| `ene-stage-poc-d` | D: Linux の OS input region / click-through |
| `ene-stage-poc-click-sink` | 実際のポインタ click を記録する別プロセス |

今回 **Windows は対象外** です。Linux のみ。X11 と Wayland は別
バックエンドとして測ります。

```sh
# 実験 C（release。既定は warmup 5 秒 + measure 12 秒 + idle 5 秒）
cargo build --release -p ene-stage-poc --bins
DISPLAY=:1 WGPU_BACKEND=vulkan ./target/release/ene-stage-poc-c

# 実験 D
DISPLAY=:1 WGPU_BACKEND=vulkan ENE_STAGE_POC_SECONDS=12 ./target/release/ene-stage-poc-d
ENE_STAGE_POC_SHAPE_KIND=both   # X11: Bounding+Input（xfwm4 では必須）
ENE_STAGE_POC_X11_TARGET=client|frame|both
ENE_STAGE_POC_OVERRIDE_REDIRECT=1
ENE_STAGE_POC_MOVE_VRM=1
ENE_STAGE_POC_REGION_PX=2 ENE_STAGE_POC_REGION_MS=16
```

---

## 環境

この Cloud Agent VM:

| 項目 | 値 |
|---|---|
| OS | Linux 6.12, Xorg 21.1.11 |
| Desktop | XFCE |
| X11 WM | **xfwm4**（compositing **on**） |
| Display | `DISPLAY=:1` |
| GPU | **`/dev/dri` なし** — ソフトウェア Vulkan のみ |
| Adapter | `llvmpipe (LLVM 20.1.2, 256 bits)` |
| Driver | `llvmpipe` / Mesa 25.2.8 (LLVM 20.1.2) |
| wgpu backend | Vulkan |
| Build | `cargo build --release -p ene-stage-poc` |
| VRM | `ene_vrm::minimal` フィクスチャ（Alicia はこの workspace にない） |
| 窓 | 800×600、透明、注記がなければ枠なし |

実 GPU は使えませんでした。検証 1 の debug+lavapipe 数値は C の
最終判断に使いません。以下の C は **release + lavapipe** です。
同じ窓・同じ VRM・同じ wgpu・同じ解像度・同じループでの **差分**
比較です。絶対 FPS は実 GPU の方が上がります。C0–C4 の順位は保たれる
想定です。

GPU ハードウェア frame time と VRAM は取れません。adapter が
`nvidia-smi` / DRM メモリを出さず、プローブも wgpu timestamp query を
有効にしていません。メモリの代理は RSS です。

---

## compositor（PoC が実際にしていること）

「zero-copy」とは書きません。内訳:

| 性質 | C0 | C1–C3（Slint） | C4（egui） |
|---|---|---|---|
| CPU readback | なし | なし | なし |
| GPU resident | はい | はい | はい |
| `copy_texture_to_texture` | なし | なし | なし |
| GPU render pass による合成 | なし | **あり** | あり（swapchain へ Load。offscreen UI なし） |

C1–C3:

```
VRM  → swapchain（clear + 3D pass）
Slint FemtoVG → offscreen Rgba8Unorm（この pass は target を clear する）
fullscreen triangle で UI texture を sample
  blend = wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING
           （src=One, dst=OneMinusSrcAlpha）
  load  = LoadOp::Load  （VRM を残す）
→ present
```

`copy_texture_to_texture` ではなく **fullscreen の alpha composite
pass** です。bind group は毎フレーム作り直します（安い）。FemtoVG は
color target を clear するため VRM pass と共有できません。

premultiplied alpha: 合成 pass は premul blend です。FemtoVG の WGPU
レンダラは premultiplied RGBA を書きます。C1（bubble 非表示）の VRM は
C0 と同じに見えるので、(0,0,0,0) の premul として扱えています。

C4 は同等の bubble を egui の `LoadOp::Load` で swapchain に直接描き
ます（本番 chrome と同じ）。Slint の offscreen は通しません。これが
ツールキット差分です。

`ene-stage-poc` のソーススキャンは合成経路の `map_async`、
`copy_texture_to_buffer`、`copy_texture_to_texture`、
`SharedPixelBuffer` を禁止します。

---

## 実験 C — 公平な compositor コスト

同じ VRM、800×600、Vulkan llvmpipe、release、`AutoVsync`。warmup 5 秒
（shader / pipeline の hitch）、その後 10 秒以上の measure、その後 5 秒
idle（`ControlFlow::WaitUntil`、redraw なし）。

最初の C 走行では idle が壊れていました。`rotate_phase` が frame リストを
空にし、`Poll` が回り続けていました。修正後、idle は 0 frame・プロセス
CPU 0 です。

### ケース

| ケース | 経路 |
|---|---|
| C0 | VRM → surface。UI renderer なし。基準値。 |
| C1 | C0 + Slint 初期化 + 空の offscreen + composite pass。bubble 非表示。 |
| C2 | C1 + 静的 bubble（半透明 rounded rect、text、button）。 |
| C3 | C2 + 軽い animation（opacity、Y 位置、scale、点滅カーソル）。 |
| C4 | C0 + 同等の egui bubble。swapchain への Load pass。 |

### 生データと warmup 除外後（measure）

`startup_ms` はプロセス開始 → 最初の present（hitch を含む）。
**hitch** は warmup の `max_ms`。**定常** は measure フェーズ。

この VM の release `ene-stage-poc-c`:

| ケース | startup_ms | warmup max_ms（hitch） | measure fps | avg_ms | median | p95 | p99 | max_ms | CPU user_ms / 12 秒 | RSS 終了 |
|---|---|---|---|---|---|---|---|---|
| C0 | 94 | 15.4 | 507 | 1.97 | 1.91 | 2.45 | 2.90 | 14.0 | 27780 | 122 MiB |
| C1 | 137 | 39.7 | 210 | 4.76 | 4.66 | 5.55 | 6.06 | 6.95 | 34890 | 130 MiB |
| C2 | 239 | 124 | 184 | 5.44 | 5.39 | 6.19 | 6.65 | 11.2 | 33170 | 149 MiB |
| C3 | 199 | 119 | 182 | 5.49 | 5.76 | 6.98 | 7.41 | 13.4 | 30490 | 155 MiB |
| C4 | 81 | 12.5 | 482 | 2.08 | 2.02 | 2.53 | 2.82 | 4.45 | 27130 | 127 MiB |

idle（全ケース）: **0 frame、CPU 0、RSS 不変**。event loop は sleep
します。

animation 中 CPU は measure の user CPU（連続 redraw）。idle CPU は
idle フェーズ（ほぼ 0 であること）。

### 差分（measure の平均 frame）

| 差分 | avg_ms | 意味 |
|---|---|---|
| **C1 − C0** | **+2.79 ms**（1.97 → 4.76） | Slint 基盤 + offscreen + composite pass |
| **C2 − C1** | **+0.68 ms** | 静的 bubble（text / rounded rect / button） |
| **C3 − C2** | **+0.05 ms** | 軽い animation |
| **C2 vs C4** | 5.44 vs 2.08（lavapipe で **2.6 倍**） | Slint offscreen+blit 対 egui の swapchain Load |

RSS: C1−C0 は約 +8 MiB（Slint + 窓サイズの RGBA）。C2−C1 は約 +19 MiB
（font / widget）。C4 は C0 に近い（+5 MiB）。

### Slint 性能の判定

**許容可能。** blocker ではない。

効くコストは C1−C0（compositor / FemtoVG 初期化）で、widget でも
animation でもありません。ソフトウェア Vulkan では +2.8 ms、VRM のみ
の約 2.4 倍ですが、まだ約 210 FPS で 60 FPS には十分余裕があります。
C2 vs C4 は「bubble 描画が 2.6 倍遅い」ではありません。C4 は
offscreen を使いません。差の大半はすでに C1 にあります。

実 GPU なら C1−C0 は縮むはずです。この VM では主張できません。将来の
製品がソフトウェア raster の 4K なら **要最適化** です。実 GPU 上の
Stage では、これだけで Slint を捨てる理由にはなりません。

---

## 実験 D — Linux の input region / click-through

OS 領域 = **UI の interactive bounds ∪ VRM の粗い bounds**（head、
torso、左手、右手）。純関数 `build_input_regions` で作ります。更新は
矩形が `ENE_STAGE_POC_REGION_PX`（既定 2 px）以上動いたとき、かつ
`ENE_STAGE_POC_REGION_MS`（既定 16 ms）以上空いたときに限る。

「窓全体を click-through にして hover で戻す」方式は使いません。

プロセス内ルーティング: UI > VRM > none。重なりは常に UI。ログ:

```
UI hit: …  VRM hit: …  OS region hit: …  target: …
```

unit test: union、優先度、overlap、背景、複数 UI / VRM、移動
threshold、AABB、rate limit、hidden UI、空領域。

OS click-through の成功は **別プロセスが click を受け取ったこと** で、
プロセス内の `Passthrough` ラベルではありません。

### X11（xfwm4 / XFCE）

SHAPE 1.1、XFixes 5.0。client は reparent されます（`XQueryTree`:
client → frame → root）。`_NET_WM_BYPASS_COMPOSITOR=1` を付けます。
reset は止まりません。

| 変種 | SET 直後 | 約 200 ms 後 | 透明部分 → click-sink プロセス？ | 透明部分 → poc-d？ |
|---|---|---|---|---|
| **client** へ Input | Input = UI∪VRM | client+frame の Input が **800×600 に戻る** | **いいえ** | はい、`target: Passthrough` |
| **frame** へ Input | frame Input = UI∪VRM。client は最初から全面 | 両方全面 | いいえ | はい、Passthrough |
| **両方** へ Input | 両方 SET | 両方全面に戻る | いいえ | はい、Passthrough |
| `override_redirect` | frame なし（parent=root） | **それでも** Input が全面に戻る | いいえ | はい、Passthrough |
| decorated | frame Bounding は装飾付きシルエット | client の Input は戻る | いいえ | （タイトルバーで座標がずれる） |
| **Input + Bounding（client）** | 両方 = UI∪VRM | Input は全面に戻る。**Bounding は UI∪VRM のまま** | **はい（`SINK CLICK`）** | **イベントなし**（UI/VRM click は届く） |

**検証 B の reset の原因:** xfwm4 の compositor は ARGB 窓の
**Shape Input** を約 1 frame で上書きします。override-redirect でも
同じです。Bounding を全面には戻しません。当たり判定は Bounding ∩
Input なので、Input が全面に戻ったあとでも Bounding の穴があれば
click は client を外れます。

この WM では Input だけの Shape は click-through API として成立しません。
**Bounding ∪ Input** なら成立します。

制約: Bounding は本当に穴を開けます（粗い矩形で、VRM のピクセル
シルエットではない）。bubble の角丸も矩形になります。毎 frame Shape を
叩いてはいけません（コストは後述）。

### Wayland（Weston 13）

入れ子 compositor: `weston 13.0.0`、**pixman** renderer、X11 backend
の 1280×800 窓、socket `ene-poc`。クライアントは
`WAYLAND_DISPLAY=ene-poc` かつ **`DISPLAY` を外して** 起動し、winit が
**native Wayland**（XWayland ではない）になるようにしました。

ログ:

```
native_wayland=true
Wayland wl_surface input region attached
server=wayland
```

`wl_surface::set_input_region` に UI∪VRM 矩形（空シーンなら空）を
渡します。

| 確認 | この Weston では |
|---|---|
| native Wayland（XWayland ではない） | はい |
| 領域内の pointer event | はい。`target: Ui`、`Vrm(Head)`、移動中の `Vrm(LeftHand)` |
| 空領域 → poc-d に event なし | はい（透明 click の `Passthrough` 行なし） |
| 空領域 → 他クライアント | weston の **desktop-shell 背景** が click を受けた。overlay の下に積んだ winit click-sink が穴の直下にいるとは限らなかった。この compositor では第二プロセスのログ成功とは書かない。 |
| 動く領域 | 既定 throttle で OS 更新 8.4 回/秒。分類は崩れない |

**Weston 13 での成功であり、Wayland 全般ではありません。** Mutter /
KWin / wlroots / Hyprland は走らせていません。

Wayland の OS apply はここでは安いです（約 12 µs。X11 Shape は数百 µs）。

### 領域更新コスト

| 走行 | OS 更新 | apply_hz | gen_avg | apply_avg |
|---|---|---|---|---|
| 静止（6 秒） | 1 | 0.17 /秒 | ~0 µs | 最初の SET 3.3 ms |
| 移動、2 px / 16 ms | 50 | **8.3 /秒** | ~0–1 µs | 0.7 ms |
| 移動、throttle なし | 2161 | **360 /秒** | 1 µs | 0.37 ms |
| 移動、4 px / 50 ms | 26 | 4.3 /秒 | ~0 µs | 0.9 ms |
| Wayland 移動（20 秒） | 168 | 8.4 /秒 | 1 µs | **12 µs** |

**毎秒 60 回の OS 更新は不要です。** 生成は AABB の union（ナノ秒）。
高いのは X11 の SET です。dirty threshold + rate limit で移動 VRM は
約 8 Hz です。本番の推奨: dirty flag + ピクセル閾値 + 30–50 ms の
rate limit。

### fallback（評価のみ）

| 層 | 提案 |
|---|---|
| Wayland | シーン幾何からの動的 `set_input_region`（Weston 13 では成立）。 |
| Shape Input が安定する X11 | Input だけの union。**xfwm4 では不可。** |
| xfwm4 など compositor 付き X11 | 粗い union を **Bounding + Input** に SET（実測で成立）。 |
| より粗い X11 | SET 頻度や見た目の穴が気になるなら Bounding を大きくする。 |
| 最終手段 | ユーザー設定で窓全体 click-through。Linux 設計を出すために必須ではない。 |

### プラットフォーム抽象

`StageInputRegion`（`update_input_region(&[Rect])`）を維持します。

- Linux Wayland → `wl_surface::set_input_region`
- Linux X11 → client の Shape **Bounding + Input**（必要な WM では frame も）
- 純関数 `build_input_regions` / `classify_pointer` はプロセス内で OS 非依存

Windows は後回しです。

---

## 最終判定

**B. 基本設計は成立する。Linux では X11 向けに platform-specific
fallback が必要（xfwm4 のような compositing WM では Bounding shape）。**

**A ではない:** xfwm4 では Input だけの Shape では click-through しない。
本番は X11 Bounding と Wayland `set_input_region` を別バックエンドとして
扱う必要がある。

**C ではない:** compositor コストは許容範囲。idle は本当に idle。
Wayland の input region は Weston で動く。X11 の click-through は
Bounding を付ければ **成立する**。プロセス内の UI > VRM > none は
unit test 済み。

| 観点 | 結果 |
|---|---|
| 性能 | Slint のコストは C1−C0。lavapipe でも 60 FPS を大きく上回る。CPU readback なし。 |
| メモリ | compositor で +8 MiB、静的 widget で +19 MiB（対 C0）。問題なし。 |
| 合成 | fullscreen premul pass。texture copy なし。readback なし。 |
| click-through | xfwm4 で Bounding+Input + click-sink プロセスにより証明。 |
| X11 | Input だけは reset される。Bounding は残る。fallback が必要。 |
| Wayland | **Weston 13** 上の native client で成立。一般化しない。 |
| 保守 | 矩形 union 1 本の API の裏に Linux バックエンドが 2 つ。妥当。 |

### この検証でやらなかったこと

- chat / detail / theme の Slint 移植なし。
- 本番オーバーレイの変更なし。
- 実 GPU の数値なし。
- Mutter / KWin / Sway / Hyprland は未走行。
- Windows は評価していない。
