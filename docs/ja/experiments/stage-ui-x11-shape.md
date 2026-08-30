# Stage UI 検証: X11 の visual と input shape

[Stage UI 検証 2](stage-ui-poc-2.md) の続きです。**製品経路ではありません。**
本番 `ene-stage` は変更していません。

コード: `crates/ene-stage-poc/`

| バイナリ | 役割 |
|---|---|
| `ene-stage-poc-x11-shape` | Experiment D2（`T1` / `T3` / `T4` / `T5` / `T5m` / `T6` / `T7u` / `T7d` / `T7o` / `input` / `both`） |
| `ene-stage-poc-click-sink` | 別プロセス。`SINK PRESS` / `SINK RELEASE` / `SINK CLICK` を `x`/`y` 付きで記録 |

この報告は **この Cloud Agent VM の xfwm4** です。他 WM は入れていません。
「X11 全体の仕様」として読まないでください。

```sh
DISPLAY=:1 WGPU_BACKEND=vulkan env -u WAYLAND_DISPLAY \
  cargo build --release -p ene-stage-poc --bins
ENE_STAGE_POC_SECONDS=8 ./target/release/ene-stage-poc-x11-shape T1
ENE_STAGE_POC_SECONDS=8 ./target/release/ene-stage-poc-x11-shape T5
```

click-through の成功は必ず両方です。overlay がイベントを受けず、かつ
`ene-stage-poc-click-sink` が受けること。

---

## 環境

| 項目 | 値 |
|---|---|
| OS | Linux 6.12, Xorg 21.1.11 |
| Desktop | XFCE |
| X11 WM | **xfwm4**（compositing on） |
| 他 WM | なし（Mutter / KWin / Openbox / i3 / bspwm なし） |
| Display | `DISPLAY=:1`（この走行では `WAYLAND_DISPLAY` を外す） |
| GPU | `/dev/dri` なし、llvmpipe / Mesa 25.2.8, Vulkan |
| Overlay | 800×600、+80+80、透明、always-on-top |
| Sink | 1000×700、+0+0、別プロセス |
| VRM | `ene_vrm::minimal` fixture |

SHAPE 1.1、XFixes 5.0。SET は `XShapeCombineRectangles` で、XFixes region
は使っていません。`ShapeClip` はどのケースでも client が
`800×600+0+0` のままです。画面上の clip は **Bounding** です。

ダンプは毎回次を出します。

```
client=0x…
frame=0x… | none
Bounding(client): …
Input(client): …
Clip(client): …
Bounding(frame): …
Input(frame): …
Clip(frame): …
effective_input_shape=…
```

D2 は frame があるとき client と frame の両方へ SET します（Experiment D
のデフォルト `ENE_STAGE_POC_X11_TARGET=client` とは違います）。

---

## 問い

X11 で

visual geometry ≠ interaction geometry

を保ったまま、glow / shadow / particle / 透明余白の click を別プロセスへ
通せるか。

理想:

- Bounding = VRM visual ∪ bubble ∪ shadow ∪ glow ∪ particles
- Input = VRM 粗い collider ∪ 対話 UI

---

## Test 1: Bounding > Input（単一矩形）

Window 800×600。Bounding `600×400+100+100`。Input `200×100+300+250`。
Bounding 内にシアン、Input 内に "CLICKABLE" bubble。VRM なし。

| 点 | Overlay | Sink |
|---|---|---|
| Input 中央（画面 480,380 → overlay 400,300） | `OVERLAY PRESS … layer=Interaction` | 新規 click なし |
| Bounding のみ（画面 230,230） | overlay イベントなし | キャンペーン走行で `SINK PRESS` |
| Bounding 外（画面 100,100） | overlay イベントなし | `SINK PRESS x=99 y=43` |

`shape::get_rectangles` は t=50 ms / 200 ms / 1000 ms / 終了で:

- Bounding(client)=`600×400+100+100`
- Input(client)=`200×100+300+250`
- `effective_input_shape=MatchesRequested`
- `wm_resets=0`

Bounding が大きな単一矩形、Input が小さな単一矩形のとき、xfwm4 は
Input を full-window へ戻しませんでした。

---

## Test 2: xfwm4 書き換え後の実効 Input

推測ではなく測定です。

| 構成 | ~200 ms 後の Input（`get_rectangles`） | click の実効領域 |
|---|---|---|
| T1 単一矩形 | 要求どおり（MatchesRequested） | 要求 Input |
| 複数矩形 Input（T3/T5/T7） | union の YX-band。**full-window ではない** | その band リスト（800×600 ではない） |
| Bounding = Input 複数矩形（`both` / T4） | Bounding と Input が同じ band（`MatchesBounding`） | そのシルエット |
| 複雑な Bounding ≠ Input（T3/T5） | Input は上記の band。Bounding は **`800×600+0+0`** | Input の band。Bounding は clip しない |

候補:

- A 元 window 全体: Bounding+Input を SET した Input はこれではない。T3/T5 の Bounding だけが full window
- B 現在の Bounding: `both` / T4 は `MatchesBounding`
- C client rect / D frame rect: Input の中身ではなかった
- E その他: 複数矩形 Input は YX-band される。bubble `277×139+40+79` は `277×48+40+79` と帯に分解される

passthrough の判定は dump 文字列ではなく実 click です。書き換え後も
glow / particle / 透明は sink へ、bubble / VRM は overlay へ届きました。

---

## Test 3 / 5: visual-only Slint + VRM

bubble + glow + shadow + 黄色い particle + `ene_vrm::minimal`。
Bounding は visual union（複数矩形）。Input は bubble ∪ VRM 粗い部品。

T5 再走（`env -u WAYLAND_DISPLAY`）:

| Click | Overlay | Sink |
|---|---|---|
| Glow（画面 100,200） | なし | `SINK PRESS x=99 y=143` |
| Bubble（画面 200,200） | `OVERLAY PRESS x=120 y=120 layer=Interaction` | なし |
| VRM（画面 480,450） | `OVERLAY PRESS x=400 y=370 layer=Interaction` | なし |
| Particle（画面 140,600） | なし | `SINK PRESS x=139 y=543` |

対話 / visual-only / 背景の 3 種は、xfwm4 が Bounding を full client に
戻したあとでも、別プロセス click で区別できます。

WM は Bounding = visual union を維持しません。Input は interaction
union の band として残します。Bounding が full window になるので GPU
の描画はそのまま見えます。

---

## Test 4: Bounding clip

Bounding を interaction と同じに SET（glow / particle / 「髪」相当は
はみ出す）。書き換え後、Bounding = Input = band された interaction
シルエット。`Clip` は full window のまま。

`720,20` の黄色い particle は GPU 上は window 内、シルエット外です。
T4 のスクリーンショットには見えません。T3/T5 では見えます。Bounding
の外の GPU 画素は X11 Bounding で clip されます。

**画面上の visual footprint = Bounding** です。glow を見せるなら
その AABB は Bounding 内に入れる必要があります。T3/T5 のように WM が
Bounding を full window に戻すと、GPU が描いたもの（透明余白含む）は
全部見えます。

---

## Test 6: ShapeNotify で Input 再適用

T6 は T5 と同じ geometry を SET し、Input が要求と違うたびに
`XShapeCombineRectangles` し直します。26.8 秒、bubble / glow / 背景を
繰り返し click。

| 指標 | 値 |
|---|---|
| `REAPPLY_INPUT` | **4052** |
| 再適用レート | **約 151 /s** |
| `ShapeNotify` Input | 連続、約 6 ms 間隔 |
| SET 時間 | 各 130–600 µs |
| `wm_reapply_fight` | **true** |

ここでの xfwm4 は Input を 800×600 に戻していません。矩形リストを
YX-band します。要求リストとの `rects_match` が毎回外れ、client が
SET、WM が band、ShapeNotify、再 SET になります。

奪い合いです。本番候補から外します。「一応動いた」では採用しません。

再適用しない場合、`wm_resets` は **1**（最初の SET の直後に一度
書き換わる）で、その後 band リストで安定します。

---

## Test 7: undecorated / override-redirect / decorated

| モード | Frame | 1 秒後 Bounding | 1 秒後 Input | click-through |
|---|---|---|---|---|
| Undecorated managed（T7u、Stage 相当） | あり | full window（複雑な union） | YX-band された interaction | glow → sink、bubble → overlay |
| override_redirect（T7o） | なし（`parent=root`） | full window | 同じ band Input | bubble → overlay |
| Decorated（T7d） | あり。frame Bounding は装飾シルエット | client Bounding は full | frame Input にタイトルバー帯 | タイトルバーで hit がずれる。Stage 向きではない |

Stage は undecorated + always-on-top です。override_redirect は分割に
必須ではありません。decorated はデフォルトにしない。

ARGB の compositor 透明度は 3 モードとも成立しました。

---

## Test 8: 他 WM

未走行。この VM にあるのは xfwm4 だけです。追加導入はしていません。

---

## 動的移動（T5m）

dirty フラグを「SET したときだけ」更新するようにしたあと:

| `ENE_STAGE_POC_REGION_MS` | bounding_hz | input_hz | combined SET µs | CPU user / 10 s |
|---|---|---|---|---|
| 30 | 14.6 | 8.4 | 795 | 25450 ms |
| 50 | 14.6 | 8.5 | 712 | 25640 ms |
| 100 | 9.7 | 7.1 | 588 | 25490 ms |

30 ms と 50 ms が同じなのは、2 px dirty 閾値が律速だからです。100 ms
でレート制限が効きます。combined SET は 1 ms 未満。約 8–15
updates/s で足りる、という Experiment D と同じ結論です。

CPU は lavapipe の redraw で、SHAPE ではありません。

T5m では bubble の Y と VRM の X が動いているあいだも、glow は sink、
bubble/VRM は overlay へ届きました。古い位置の click-through 穴は、
その click では出ていません。T5m の Bounding は full window に戻るので、
visual clip は制約になっていません。

---

## 結果表

この xfwm4 セッションと click-sink プロセスの値です。

| Configuration | Visual-only visible | UI clickable | VRM clickable | Visual-only click-through | Background click-through | Stable under xfwm4 |
|---|---|---|---|---|---|---|
| Input only | はい（Bounding は full window） | YX-band Input 内なら | YX-band Input 内なら | その外ならはい | はい | 要求どおりではない。Input は書き換えられる |
| Bounding + Input（同じ矩形） | band シルエットに clip | はい | はい | **いいえ**（Bounding 内 glow は click される） | はい | Bounding と Input が同じ band で残る |
| Bounding > Input（T1 単一矩形） | はい、Bounding 内 | はい、Input | n/a（VRM なし） | **はい** | はい | **はい**（`MatchesRequested`） |
| Bounding > Input（T3/T5 union） | はい（WM が Bounding を full client に広げる） | はい | はい | **はい**（glow / particle → sink） | はい | Input は band、Bounding は full。使えるが要求 union ではない |
| ShapeNotify reapply | T5 と同じ | はい | はい | はい | はい | **いいえ**（約 151 reapply/s の奪い合い） |
| undecorated | はい | はい | はい | はい | はい | T5 と同じ |
| override_redirect | はい | はい | はい | はい（キャンペーン） | はい | frame なし。Input は band |

---

## X11 architecture 候補

**B。** Bounding + Input は使えます。xfwm4 でも visual-only の
click-through は成立します。ただし Bounding を複雑な union として安定
維持することはできません。

制約:

- visual footprint で clip したいなら **Bounding は 1 個の AABB**（T1）。
  複数矩形 Bounding は full client に戻された。
- Input は YX-band される。hit はその前提か、Input も AABB 1 個にする。
- 見える必要がある glow / shadow / particle は Bounding 内。はみ出しは
  T4 で clip される。
- ShapeNotify での Input 再適用はしない。

A ではない: 「Bounding = visual union、Input = interaction union、両方
要求どおり維持」は **重ならない単純矩形** でしか成立しませんでした。

C ではない: Input を Bounding より小さく保てます。T1 はきれいです。T5
は Bounding が full window になっても visual-only が sink へ通る。

D ではない: SHAPE は WM 依存ですが、xfwm4 で helper window を足す理由に
はなりません。helper-window fallback の PoC は **していません**。

---

## 最終判定

**B。分割はだいたい成立する。xfwm4 では X11 固有の fallback が要る。**

| 基準 | 根拠 |
|---|---|
| 別プロセス passthrough | T1 の Bounding のみと T5 の glow/particle: overlay 無反応、sink が `PRESS`/`RELEASE` |
| visual clip | T4 で particle が画面に無い。Clip は未使用。clip は Bounding |
| WM 介入 | Input の YX-band。複雑な Bounding → full window。一度書き換わったあと、奪い合いしなければ安定 |
| 動的移動 | 8–15 SET/s。Bounding と Input の dirty は独立 |
| 性能 | SET は 1 ms 未満。50–100 ms の rate limit で足りる |
| 複雑さ / 保守 | StageWindow は 1 枚。再適用ループは禁止。この WM では helper window なし |

本番の X11 設計は、Bounding を visual AABB にする（または compositing WM
では full-window Bounding を受け入れる）、Input を interaction にする、
YX-band を契約の一部にする、です。Mutter / KWin では測り直すまで
「Linux X11」と書かないこと。

---

## 純関数テスト

`crates/ene-stage-poc/src/region.rs` の `build_visual_region` /
`build_interaction_region`。対象: visual > interaction、visual-only
glow、hidden UI、VRM 移動、interaction 移動、空、複数コンポーネント、
閾値、独立 dirty、実効 Input の分類。

---

## この検証でやらなかったこと

- 本番 `ene-stage` の変更なし
- chat / detail / theme の Slint 移植なし
- helper-window の試作なし（B で足りた）
- Mutter / KWin / Openbox / i3 / bspwm なし
- Windows なし
- 入れ子 Weston の `WAYLAND_DISPLAY=ene-poc` が残っていると winit が
  Wayland client になり、この検証は無効
