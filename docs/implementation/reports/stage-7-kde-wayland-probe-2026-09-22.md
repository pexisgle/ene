# Stage 7: KDE Wayland 実環境 probe / Linux Performance Gate 報告

実施日: 2026-09-22
対象 tip: `24dd163cd1b62a6eb5efc7b6065e09a8d00a6091` + 本報告の probe / 修正（作業 tree）
環境: NixOS 26.11.20260911.eaad089 / KDE Plasma 6.7.5 / KWin 6.7.5 Wayland /
AMD Radeon RX 9060 XT (Mesa 26.2.2, radv) / Linux 7.2.5 / HDMI-A-1 + DP-1 (各 1920x1080, scale 1)
文書の位置づけ: **技術成立 probe と Performance Gate の実測記録**である。公式 `ene` VRM (#1651)
と正式リリース NixOS 26.11 の最終 acceptance を代替しない。

## 結論

- KDE Wayland 実セッションで layer-shell overlay の表示、alpha-aware input region による
  click-through、`wp_presentation` に基づく presented evidence を実測した。
- 実環境で 5 件の実装欠陥を発見し、修正した:
  presentation output が空 (KWin は `sync_output` を送らない)、clean exit の Mesa WSI
  segfault (drop 順)、Wayland input region が固定矩形、**hide/restore が layer-shell の
  remap protocol error で復帰不能**、Linux の clippy dead_code gate 不全。
- Linux Secret Service (ksecretd / `org.freedesktop.secrets`) への実 OS store
  put / read-back / 不変性 / activate / delete を実測し、成功した。
- 日本語 IME (fcitx5 + mozc) の未確定入力が送信されないことを製品 GUI で確認した。
- Performance Gate (Linux, release, 5 分 idle) は **Pass**: CPU 2.336% machine /
  0.280 core-equivalent / busy-wait なし、RSS peak 247,468,032 B (≤ 2 GiB)、presented
  31.100 FPS / 311 frames / discarded 0 / missing 0、cancel 受付 38.2 ms / 描画 40.8 ms、
  click-through 1 ms。Body は `assets/seed-san.vrm`（開発 sample）で、公式 `ene`
  (#1651) ではない。

## 環境の記録

| 項目 | 値 |
| --- | --- |
| OS | NixOS 26.11.20260911.eaad089 (x86_64-linux) |
| Desktop | KDE Plasma 6.7.5 / KWin 6.7.5 Wayland |
| Wayland globals | `zwlr_layer_shell_v1` v5, `wp_presentation` v2, `zwp_text_input_manager_v3` v2 |
| GPU | AMD Radeon RX 9060 XT, radv, Mesa 26.2.2, DRM 3.64 |
| Display | HDMI-A-1 1920x1080 @100%, DP-1 1920x1080 @100% |
| IME | fcitx5 5.1.21 + mozc, layout jp |
| Secret Service | ksecretd / KWallet 6.30.0 (`org.freedesktop.secrets` on session bus) |

## D: overlay / 表示 / click-through / hide / resize / HiDPI

### probe tool (検証用、製品ではない)

`apps/ene-body/src/bin/ene-body-overlay-probe/`
- `main.rs`: Body runtime を in-process で起動し、製品と同じ projection IPC で
  AssetRef / Placement / PoseHint / Show / Hide を送り、Body→parent の全 event を
  JSONL で出力する。
- `underlay.rs`: 同じ compositor 上に Top layer の別 surface を作り、overlay が
  入力領域として主張しない点に届いた実 pointer event を JSONL に記録する
  (「別 surface が透明領域の click を受けた raw log」)。

### layer-shell overlay の表示

- 製品 `ene-body` を KWin Wayland 上で起動し `Ready { overlay: KdeLayerShell,
  gpu: Ok }` / `AssetReady { primitives: 21, expressions: 18, spring_chains: 9 }` を確認。
- 透明ウィンドウとして実表示され、`spectacle` の window capture で avatar の描画を確認。
- KWin の window 一覧で `Position: 24,1104 Geometry: 420x640` を確認。

### click-through (alpha-aware input region)

- **修正前**: Wayland backend の input region は placement box の 75%×95% の固定矩形で、
  avatar の透明部分も click を奪っていた。
- **修正**: presented frame の `HitTestMask` から `wl_region` を構築し、visible pixel のみを
  入力領域にする。mask を可視化すると T-pose のシルエットそのもの (rects=87, union
  x16..404 y188..452)。resize grip は Windows と同じ「下端 band の rightmost visible
  cell」基準に変更した。mask が空なら入力領域も空 (完全 click-through)。
- 実測 (`points-final.jsonl`, raw: `underlay-final.jsonl`):

| local point | 結果 |
| --- | --- |
| (144,250), (144,300), (144,400) (avatar 上) | overlay が受領 (underlay に届かず) |
| (200,300), (250,300), (350,500) (透明) | underlying surface へ到達 (0–1 ms) |
| (20,20), (400,20), (20,620), (400,620) (四隅) | underlying surface へ到達 (0–1 ms) |

- 最大 input block `0.001 s`。raw evidence は
  `/tmp/opencode/probe2/evidence/underlay-final.jsonl` (press の座標・`t_unix_ns` 入り)。

### wp_presentation に基づく presentation evidence

- KWin 6.7.5 は `wp_presentation_feedback.sync_output` を送らない (WAYLAND_DEBUG で
  presented 177 件 / sync_output 0 件) ため、修正前は `output` が空で
  `PresentationRecord::passes()` が必ず落ちていた。
- **修正**: `wl_surface.enter` で得た出力を `sync_output` 不在時の attribution とした。
- 修正後: `Presented { clock_id: 1 (MONOTONIC), output: "DP-1", timestamp_ns }` を実測
  (`presentation-final.jsonl`, 311/311 resolved, discarded 0, missing 0 in the gate window)。

### hide / restore

- 実測で **hide 後に復帰不能** を発見した。`attach(None)` + commit で unmap したあと、
  KWin は次の buffer attach より前に新しい configure を要求するが、null-buffer commit
  には configure を送らない。再表示時は configure 未受信のまま attach するため
  protocol error 0 ("a buffer has been attached to a layer surface prior to the first
  layer_surface.configure event") で Body の Wayland 接続が死に、
  `GpuFail(DeviceLost)` のまま復帰しなかった。
- **修正**: hide は「透明 frame を 1 枚 present して mapped を維持し、以後 present を
  止める」方式にした。透明 frame により alpha-aware input region も空になり
  click-through になる。unmap しないため configure 再取得の問題が発生しない。
- 実測 (probe): hide 前 443 presented → hide 後 +2 で停止、show 後 31 fps で復帰、
  protocol error 0 件、`gpu_ok: true` 継続。
- 製品 GUI でも確認: 管理画面「一般 → デスクトップアバター」の
  「一時的に隠す」で presented +0 (停止)、「表示」で +93/3 s (約 31 fps) に復帰。

### resize (Placement / pointer grip) と drag

- `Placement { x, y, width, height, scale }` の変更で window が 600x800 に再構成され、
  presented は +62/2 s で継続。
- pointer resize: mask の下端 band の rightmost visible cell (local (235,585)) を
  press して drag → `LocalUi Resize {width: 650, height: 850}`、window 650x850。
- pointer drag / resize は `zwp_relative_pointer_v1` の加速付き delta で行う。
  修正前は surface-local 座標を press 時の固定アンカーと比较していたため、surface 自身の
  移動が feedback して移動量が半減し、commit と event の交錯でガタついた。実測 (probe):
  - 修正前: 相対入力 (200,120) に対し surface は (100,60) しか動かず、LocalUi も不連続。
  - 修正後: cursor delta (200,120) = surface delta (200,120)。逆方向 (-96,72) も一致。
- drag: `LocalUi Drag` が placement 座標で連続的に増加し、window が (124,1144) へ移動。

### HiDPI (scale 2)

- 入れ子 KWin 6.7.5 (`--wayland-display wayland-0 --socket ene-nested --width 1280
  --height 720 --scale 2`) の出力 `WL-0` (logical 640x360) で overlay を実行:
  `Ready { overlay: KdeLayerShell, gpu: Ok }`、`Presented { clock_id: 1, output:
  "WL-0" }` が継続し、protocol error 0 件。scale=2 の buffer/buffer_scale と
  logical 座標の input region で動作した。
- 入れ子 compositor の window capture (`nested-crop.png`) で avatar が 2x で描画される
  ことを確認。
- 物理 display の scale 変更は行っていない (session を壊さないため)。実解像度の
  HiDPI acceptance は 26.11 最終 acceptance の対象。

## Linux Secret Service (実 OS store)

`crates/ene-credential` の実機 round-trip test を実行:

- 結果: **PASS** (`the_real_os_store_round_trips_when_available`)
- 確認内容: version item の put、read-back、version ごとの別 item、published version の
  上書き拒否 (immutability)、activate 後の snapshot 読み、retire version の delete、
  active version の delete。
- store: ksecretd (KWallet) の `org.freedesktop.secrets`。namespace は
  `ene-probe-<uuid>` を使い、終了時に両 version を削除。生値はログに出していない。
- 製品経路でも GUI から credential を登録し、`credential_*` durable 更新と
  セットアップ完走を確認した (下記)。

## IME (fcitx5 + mozc, 製品 GUI)

製品 GUI (Host が起動した `ene-desktop`) のチャット入力で確認:

1. mozc で "konnichiwa" を入力 → **未確定** (preedit こんにちは)。この間
   provider 呼出しは 0 件 (`ime-preedit.png`)。
2. Enter で確定 → 入力欄に確定文字列が入る。**送信されない** (provider 0 件,
   `ime-committed.png`)。
3. 確定後にもう一度送信 → 送信され、provider 呼出しが 1 件発生 (`ime-sent.png`,
   fake provider JSONL)。

## 製品セットアップ (computeruse)

- Host-spawned GUI の本人確認 (端末接続)、wizard 1/5〜5/5、credential 登録
  (OS store へ publication)、model 割り当てを GUI から実施。
- credential 登録は「入力 → 本人確認 → 操作を受け付けました」の順で完了し、
  `credential_active` / `credential_mutation` が durable に更新された。
- Workspace folder の選択 (`select-workspace` intent, outcome `applied`) を行い、
  会話からの Task proposal が成立した。

## Performance Gate

計測方法: `ene-measure` + 実機 desktop (Host / Desktop / Body の 3 PID、release、40 桁
SHA)、`--wayland-feedback-jsonl` (Body の `wp_presentation` presented)、
`--interaction-jsonl` (GUI cancel の input→Host intake→paint)、
`--click-through-json` (実 compositor の raw evidence)、`--environment-json`。
5 分間の idle 区間で CPU / RSS をサンプルし、warmup 後 10 秒の presented FPS を評価する。

### 第 1 回 (計測経路の欠陥を発見)

- CPU 2.253% / RSS peak 222,674,944 B / presented 31.100 FPS だが **missing 1** → Fail
- 原因: Desktop は Body の presentation event を 250 ms 周期の tick で trace に書くため、
  窓内で commit した最終 frame の Presented 行が窓外の `observed` 時刻で書かれ、
  `read_wayland_feedback` が「未解決」と誤判定していた (該当 ID 52377 は Submitted の
  250 ms 後に Presented が記録済み)。display の欠測ではない。
- **修正**: 窓は commit (Submitted) の時刻で定義し、窓内 commit の terminal feedback は
  `observed` 時刻で除外しない。未解決 feedback は terminal 行が無いため従来どおり
  Missing のまま (gate を弱めない)。bin に回帰 test を追加。

### 第 2 回 (修正後・最終コード) — PASS

- Verdict: **Pass**
- CPU: 2.336% machine / 0.280 core-equivalent / busy-wait PIDs なし
- RSS: mean 247,411,776 B / peak 247,468,032 B (≤ 2 GiB)
- FPS: 31.100; presented 311; discarded 0; **missing 0**; source
  `WaylandWpPresentation { body_pid, surface_id }` (wall 10 s)
- Interaction `cancel_task`: intake 0.038245 s / painted 0.040823 s
- Click-through: reached true / max block 0.001 s
- PID 別: Host CPU 0.01 s / RSS 19.0 MB、Desktop CPU 0.76 s / RSS 81.3 MB、
  Body CPU 83.31 s / RSS 147.1 MB
- 生データ: `result3.json` / `result3.txt`、`presentation-final.jsonl`、
  `interaction-final.jsonl`、`click-through.json`、`environment.json`
  (いずれも `/tmp/opencode/probe3/evidence/`, click-through のみ
  `/tmp/opencode/probe2/evidence/`)。

## 発見した実装欠陥と修正

1. **presentation output が常に空** — KWin は `sync_output` を送らない。
   `surface_enter` 出力を fallback に追加 (unit test あり)。
2. **Body clean exit で segfault** — `WaylandOverlay` の field drop 順が
   connection → renderer で、Mesa WSI が破棄済み `wl_display` に proxy destroy を
   発行していた。`renderer` を先頭に移動し clean exit 0 を確認。
3. **透明領域が click を奪う** — Wayland の input region を固定矩形から alpha-aware
   mask に変更。resize grip も mask 基準へ。
4. **hide/restore が復帰不能** — unmap 後の remap で configure を待たず attach し
   protocol error。透明 frame 1 枚で mapped を維持する方式に変更 (上記)。
5. **drag / resize が 1:1 で追従しない** — surface-local 座標が移動する surface 基準に
   なる feedback で、持続 drag で移動量が半減・ガタつき。`zwp_relative_pointer_v1` の
   加速付き delta をアキュームレートする方式に変更 (cursor delta = surface delta)。
6. **Linux clippy の dead_code gate 不全** — Windows 専用の hit-test API を
   `#[cfg(any(windows, test))]` に分離 (Wayland が `HitTestMask` を実際に使うように
   なったため大半は解消)。
7. **FPS 計測の窓境界欠陥** — 上記 Performance Gate 第 1 回の項。

## 未実施 (成功と扱わない)

- 公式 `ene` VRM (#1651) の runtime / 見た目 acceptance。本測定の asset は
  `assets/seed-san.vrm` (VRM 1.0 sample、再配布可能) である。
- 正式リリース NixOS 26.11 での最終 acceptance (本測定は 26.11.20260911 開発版)。
  26.11 が存在しないため、この probe の記録を最終 acceptance としない。
- 物理 display の scale=2 での HiDPI acceptance (入れ子 KWin でのみ確認) — #1678。
- motion pack (`.vrma`) の実描画 (`HealthTick.motion: Unsupported` のまま) — #1679。
- IME 確定後の送信が発火しない疑いの切り分け — #1680。
- Windows 11 (既存報告の範囲)。

## Issues

- #1677: 本報告で発見した実装欠陥 6 件の修正 land (未 commit の変更、レビュー待ち)。
- #1678: 物理 display scale=2 の HiDPI 実機検証が未実施。
- #1679: motion pack (`.vrma`) の実描画 acceptance が未実施。
- #1680: IME 確定後の送信が発火しない事象の切り分け。
