# Stage UI PoC の結論

追跡: [#1260](https://github.com/pexisgle/ene/issues/1260)、プローブ [#1258](https://github.com/pexisgle/ene/pull/1258)。
英語: [experiments/stage-ui-poc-findings.md](../../experiments/stage-ui-poc-findings.md)。

生の `ene-stage-poc` クレートは `main` に入れません。このページが Stage v2 の
判断ログです。後続 issue は Adopt 行を制約として扱い、再調査しません。

## 実験

| プローブ | 結果 | 判断 |
|---|---|---|
| A/B shared wgpu + Slint 合成 | 同一 `Instance` / `Device` / `Queue`。プロセス内 UI → VRM → passthrough の経路を単体テスト済み。 | 採用 |
| C compositor コスト（Vulkan、800×600、release） | フルスクリーン premul pass、`LoadOp::Load`。CPU readback なし、`copy_texture_to_texture` なし。idle は 0 frames / 0 CPU。Slint オーバーヘッドはプローブ GPU で許容。 | GPU 常駐合成を採用 |
| D Wayland input region（Weston 13 nested/native） | `wl_surface::set_input_region` は領域内イベントを届け、空ガラスのイベントは届けない。hover-rearm 不要。 | native Wayland で採用。全 compositor の保証にはしない。 |
| D2 X11 SHAPE（xfwm4、compositing on） | Input-only SHAPE はリセットされる。Bounding > Input の単純矩形は残る。複雑な Bounding union は full client に広がることがある。Input は YX-banded。Bounding 外の画素は clip される。ShapeNotify reapply は WM と競合する（約 151 SET/s）。 | 粗い Bounding + Input を採用。ShapeNotify reapply は不採用。pixel-perfect silhouette は不採用。 |

`/dev/dri` の無いマシンでのソフトウェアラスタライザ数値は本番の
performance gate に使わない。software-reference としてだけ記録する。

## 採用

- Slint + shared wgpu 29。`unstable-wgpu-29` は renderer 境界に隔離する。
- Slint は offscreen `Rgba8Unorm` に描く。
- フルスクリーン三角形、`PREMULTIPLIED_ALPHA_BLENDING`、`LoadOp::Load` で合成する。
- CPU readback presentation は使わない。
- idle: 変化が無いとき continuous redraw しない。
- Wayland: `InteractionGeometry` → `wl_surface::set_input_region`。dirty + 数 px threshold + rate limit（移動中おおよそ 8 Hz）。hover-rearm しない。
- X11: `VisualGeometry` → 粗い Bounding（少数 AABB + effect padding）。`InteractionGeometry` → 粗い Input。runtime sanity check と window-wide fallback。
- Windows: 現行 DX12 / DirectComposition / wgpu StageWindow と window-wide `Window::set_cursor_hittest()` を維持する。先に `Passive / Interactive / Dragging / UiFocused` を明示する。
- VRM body-part hit-test は window architecture と独立。第一候補は bone 由来 CPU collider / 粗い screen-space region。

## 不採用

- GPU readback presentation。
- Windows の layered full-frame、helper HWND、small/tight-window architecture。
- Windows の cross-process partial click-through。
- X11 Input-only SHAPE を既定にする。
- X11 ShapeNotify reapply。
- X11 の pixel-perfect visual/input silhouette。
- `override_redirect` を Stage の既定にする。
- `crates/ene-stage-poc` を本番へ取り込む。
- Stage production gate（#1273）が安定する前に Chat / Detail の本格 Slint 移植を始める。

## Regression checklist

後続 PR は [stage-v2-baseline.md](stage-v2-baseline.md) と比較する。

- 透明度（premultiplied swapchain。未対応なら overlay を隠す）
- always-on-top
- platform 入力（Windows window-wide hittest、Wayland input region、X11 SHAPE + fallback）
- VRM hover / click / drag
- display-only UI は Passive のまま。clickable UI は最初の click を受け取れる
- DPI / resize
- 複数アバター
- 変化が無いとき idle redraw が止まる
