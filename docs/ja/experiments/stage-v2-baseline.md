# Stage v2 移行前 baseline

追跡: [#1261](https://github.com/pexisgle/ene/issues/1261)。
英語: [experiments/stage-v2-baseline.md](../../experiments/stage-v2-baseline.md)。
判断: [stage-ui-poc-findings.md](stage-ui-poc-findings.md)。

Slint 導入前の Stage を固定し、後続 PR が regression と既存不具合を分けられる
ようにする。renderer は変えない。

## 測定対象の commit

`cursor/stage-v2-slint-migration-345a` は `origin/main` の `8d970ce8`
（`fix(ci): drop unused Windows tray imports and rustdoc crate link`）から分岐。

## Event loop / idle redraw（コード上の事実）

`ene-stage` は `app::run` と `StageApp::about_to_wait` で
`winit::event_loop::ControlFlow::Poll` を設定する。`about_to_wait` は毎回
`tick_overlay` と `paint_chrome` を呼ぶ。

GPU に依存しない帰結:

- 現状のプロセスは **0 frames で idle しない**。
- 「idle CPU ≈ 0 / 変化が無いとき 0 frames」は Stage v2 の **目標**であり、
  現在の測定値ではない。
- WaitUntil 導入後も、idle VRMA や spring bone がある overlay はフレームを
  要求してよい。viseme / look-at / dirty Slint が無い静的ポーズは要求しては
  いけない。

## `cargo test -p ene-stage`

Cloud Agent VM（Linux、rustup stable、Nix なし）で同一 commit を反復する。
GUI 用の software Vulkan（lavapipe）はあるが、この表は単体テストだけ。

| 回 | 結果 | メモ |
|---|---|---|
| 1 | 273 passed, 0 failed（`cargo test -p ene-stage`） | Cloud Agent VM、rustup stable。この回に flake なし。 |
| 2 | 273 passed, 0 failed | 同一 commit の反復。 |
| 3 | 273 passed, 0 failed | 同一 commit の反復。 |

async timing が疑わしいテスト（名前を記録する。Stage v2 PR でまとめて直さない）:

- `ene_stage::app::tests` のうち `AsyncOutcome` を drain し session / history
  の reconcilation を待つもの（`stale_*`、`reconciliation_*`、`completion_*`）。
- `ene_stage::detail::tests` のうち stale な MCP / job 結果を無視するもの。

後続 PR がこの系統を **毎回** 落とすなら新しい regression。同程度の間欠失敗は
baseline。

## Performance（記録する。software GPU では gate しない）

| 指標 | Windows（実 GPU） | Linux（実 GPU） | Cloud VM（lavapipe） |
|---|---|---|---|
| overlay 最初のフレームまでの startup | #1261 のホストで測定 | #1261 のホストで測定 | software-reference のみ |
| Stage 起動後、pointer / speech なしの idle CPU | 測定 | 測定 | `ControlFlow::Poll` のため 0 を期待しない |
| バンドルアバター 2 体ロード後の RSS | 測定 | 測定 | software-reference のみ |
| VRM steady-state frame time（idle motion） | p50/p95/p99 | 測定 | software-reference のみ |
| overlay 非表示 / アバター無しの idle redraw | 現状は Poll のまま | 同じ | 同じ |

実 GPU 列は開発マシンが取れたときに埋める。lavapipe の差分だけで
#1265 / #1273 / #1281 を落とさない。

## 現状の platform 入力

| Platform | Overlay click-through | Hover re-arm | メモ |
|---|---|---|---|
| Windows | window-wide `Window::set_cursor_hittest`（winit → `WS_EX_TRANSPARENT`） | Stage には無い。Passive 窓は pointer を受け取らない | この architecture を維持する。 |
| X11 | `set_cursor_hittest` は no-op。`cursor_poll` が root pointer を読む | あり、50 ms | Stage v2 は残る環境で粗い SHAPE に置き換える。 |
| Wayland | input region 無し。click-through 中は pointer が来ない | なし | ドラッグするには click-through を切る。Stage v2 が `set_input_region` を足す。 |

`platform::apply_click_through`（HWND `EXSTYLE` の手書き）は未使用。本番の
hit-test 変更は `OverlayWindow::set_click_through` だけ。

## 後続 issue の regression checklist

#1265、#1269、#1273、#1281 で使う。

1. 透明 overlay が premultiplied alpha を出するか、未対応なら隠す。
2. always-on-top が `desktop.always_on_top` と chrome focus の下げに従う。
3. Windows Passive で背後のデスクトップが click を受け取る。
4. Windows Interactive / Dragging / UiFocused が最初の pointer を受け取る。
5. Wayland: `InteractionGeometry` の外の click が Stage に届かない（Weston 13。他 compositor は未保証）。
6. X11: 対応 WM で UI/VRM click が Stage に届く。visual-only / 背景 click は別プロセスに届く。ShapeNotify の奪い合いは無い。
7. VRM drag / click / double-click / long-press が `GestureTracker` で分類される。
8. display-only overlay UI が Interactive を強制しない。
9. resize / DPI で黒画面や clip が残らない。
10. 表示アバター 2 体の load / hit-test / 位置の永続化が残る。
11. WaitUntil 後、静的な Stage が event loop を回し続けない。
12. このページとの RSS / frame-time 差分を記録する（実 GPU）。
