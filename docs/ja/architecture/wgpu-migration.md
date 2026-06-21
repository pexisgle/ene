# `ene-desktop`: Bevy → wgpu / winit / egui 移行プラン

> **ステータス:** 承認済み設計 — 部分実装済み。
> **スコープ:** `apps/ene-desktop-v2` (新設、winit + wgpu) と `crates/ene-vrm` (新設、スタブ)、および既存 Bevy ベースの `apps/ene-desktop` (移行中、依然として残置)。`ene-core`, `ene-memory`, `ene-cli`, ツールバイナリは変更しない。
> **担当 / 推進:** 未定
> **到達目標:** デスクトップアプリの長年の 2 件の不具合を解消し、我々が完全に所有するスリムで保守性の高い描画スタックを実現する。

---

## 0. 現在の実装状況

本ドキュメントは移行の **設計プラン** である。以下の表は執筆時点でディスク上に実際にあるものをまとめたもので、読者がワークスペースを diff しなくてもどのフェーズが降りているか把握できるようにしている。

| フェーズ | プラン参照 | 状態 | メモ |
|----------|------------|------|------|
| **PR0 — `ene-desktop-v2` スケフォールド + Windows 透過スモーク** | §22.3 | **出荷済み** | `apps/ene-desktop-v2/` (`main.rs`, `gpu.rs`, `runtime.rs` の 3 ファイル、約 840 行) が当初計画した 7 モジュール分割を置換済み。`DxgiFromVisual` + `WS_EX_NOREDIRECTIONBITMAP` + `CompositeAlphaMode::PreMultiplied` の透過レシピ。`Space` で透過切替、`Escape` で終了。開発者の Windows 機での動作確認済み。 |
| **PR1 — v2: トレイ + AI ブリッジ + AppState + 永続化 + CLI** | §4 PR1 | **出荷済み** | 7 ファイル・~1.5k LoC (`+ state.rs, events.rs, settings.rs, ai_bridge.rs, tray.rs`)。レガシー `apps/ene-desktop` (Bevy 0.18) は意図的に無修正のまま。 |
| **PR2 — v2: 設定 UI 3 ページ + ホットキー + キャラクター別設定** | §4 PR2 | **出荷済み** | `settings_ui/` 5 ファイル・サブツリー、`character_state.rs` に PR2 スタブ。F1 / WASD / Space / Esc ホットキー、`PermissionRequired` / `UserInputRequired` で自動ポップアップ、6 個の表情テストボタンが `EmotionQueue` を push。 |
| **PR3 — v2: ortho 3D カメラ + `ene-vrm` 静的描画 (MToon + スキニング)** | §4 PR3 | **出荷済み (MVP)** | `crates/ene-vrm/src/{lib,error,camera,model,renderer,loader,shaders/mtoon_lite.wgsl}.rs` (7 ファイル)。GLB 専用ローダ (`gltf 1.4`)、単一 ortho カメラ、深度テスト付き PBR-lite WGSL シェーダ (half-Lambert + ベースカラー + プリマルチプライド アルファ)、identity スキニング。`assets/characters/Alicia/AliciaSolid.vrm` を描画。**後回し**: フル MToon (rim / matcap / outline / emission)、実スキニング算術、マルチメッシュ / マルチマテリアル、非 GLB VRM、アニメーション、表情、LookAt、ドラッグ、Spring Bone。 |
| **PR4 — v2: LookAt / カーソル / 表情 / ドラッグで移動** | §4 PR4 | **Shipped** (PR4.1 ModelUniform + カリング / **PR4.2 LookAt カーソル射影 + BodyTracking プロファイル** / **PR4.3 ドラッグで移動** / **PR4.4 表情** / **PR4.5 スキニング (rest-pose パレット)** / **PR4.6 quick-win ハードニング** / **PR4.7 humanoid bone registry** / **PR4.8 `VRMC_vrm.lookAt` パース + 毎フレーム評価器** / **PR4.9 表情オーバーライド + isBinary** / **PR4.10 アルファモードソート + 2 パス描画** / **PR4.11 `KHR_materials_unlit` 別パイプライン** (issue #19) / **PR4.12 `VRMC_node_constraint` パース + 評価器** (issue #15) / **PR4.13 `VRMC_springBone` パース + verlet シミュレータ** (issue #13) / **PR4.14 `VRMC_vrm_animation` (VRMA) パース + 再生エンジン** (issue #14) / **PR4.15 フル `VRMC_materials_mtoon` パース + per-material uniform + MToon シェーダパイプライン** (issue #12) / **PR4.16 評価器からの per-joint 骨回転 + skin-palette アップロード** — PR4 クローズ) | `apps/ene-desktop-v2/src/look_at.rs` がレガシー `update_cursor_look_target` (435–478 行) + `body_tracking_for_strength` (514–537 行) を 1:1 移植。`Runtime` が `WindowEvent::CursorMoved` を `last_cursor_logical` に記録し、`RedrawRequested` で `CharacterRenderer::update_look_at(cursor_logical, viewport, character_position, model_scale, strength, dt_secs)` がスムージングを進める。`look_at_target()` は PR4.5+ スキニングが humanoid 骨回転駆動に使うため公開済み。**PR4.8** で `crates/ene-vrm/src/look_at.rs` を追加 (`LookAtType { Bone, Expression }`、`LookAtRangeMap { input_max_value, output_scale }` デフォルト 90→10、`LookAtRangeMapSet { horizontal_inner, horizontal_outer, vertical_down, vertical_up }`、`LookAtProperties { offset_from_head_bone: [f32;3], range_map, look_at_type }` 仕様デフォルト `[0, 0.06, 0]` / 90→10 / `"bone"`、`LookAtEvaluator` + `LookAtDirection { yaw_degrees, pitch_degrees }` + `LookAtBoneDelta { delta: Quat }` / `LookAtBoneOutput { head, left_eye, right_eye }` (`"bone"` 型モデル用) + `LookAtExpressionOutput { look_up, look_down, look_left, look_right }` (`"expression"` 型モデル用))。ローダが `look_at::load_look_at` を呼び、結果を `VrmModel::look_at: Option<LookAtProperties>` に格納。`calc_yaw_pitch` は `bevy_vrm1::calc_yaw_pitch` (222–237 行) を 1:1 で反映: yaw は `atan2(x, z)` (正 = モデルが自身の左を見る)、pitch は `atan2(-y, xz)` (正 = 下を見る)。頭の世界座標は humanoid registry の `head` 骨から取得 (`character_position + head.rest.translation * model_scale`)、humanoid メタのない legacy VRM 0.x モデルは 1m Y オフセット定数にフォールバック。`CharacterRenderer` が毎フレーム evaluator を実行: `"expression"` 型は per-expression 重みを `VrmModel::expressions_mut()` に書き込み (legacy `bevy_vrm1` が silent に no-op していたパス)、`"bone"` 型は per-bone `Quat` デルタを `CharacterRenderer::look_at_bone_output` に退避 (次の skin-palette パス用)。骨回転は PR4.9。**PR4.3 ドラッグで移動** は新 `apps/ene-desktop-v2/src/character/drag.rs` (レガシー `character_drag/mod.rs` の 1:1 移植)。ヘルパ: `aabb_world_corners`、`transformed_aabb_bounds`、`ray_intersects_aabb` (スラブ テスト、レガシーと同一)、`cursor_logical_to_world_2d` (カメラ view-z=0 平面への ortho 射影、`look_at::compute_world_target` パターンに合流)、`cursor_over_character` (カーソル レイ vs. 変形後 AABB ヒット テスト)。ステート マシン: `on_press_or_release` (`DragAction::None / Started / Ended` を返す) + `tick` (積分用 `Option<Vec3>` デルタを返す)。ランタイム配線: `WindowEvent::CursorMoved` が `drag::tick` を呼びデルタを `settings.character_state.character_position` に積分; `WindowEvent::MouseInput { Left, Pressed / Released }` が `drag::on_press_or_release` を呼び; `Ended` 時に `settings.mark_dirty()`。**クリックスルーは意図的にスコープ外** (PR5) — キャラクター ウィンドウ全面がまだクリック可能。**PR4.4 表情** は `crates/ene-vrm/src/expression.rs` (新規、`ExpressionName` / `PrimitiveMorphs` / `ExpressionLayer` / `PrimitiveMorphMeta` ユニフォーム = 16 個のパック済み `vec4` 重みスロット = プリミティブあたり最大 64 ターゲット)、`resolve_expression_names` (VRMC_vrm エクステンション ウォーカー)、バインドグループ `(3)` (storage + uniform)、`shaders/mtoon_lite.wgsl` への `if (target_count > 0u)` 早期脱出、`EmotionQueue` → `CharacterRenderer::apply_emotions` のランタイム配線を出荷。**PR4.6 quick-win ハードニング** は 6 件の Issue を閉じた: `MeshVertex::joints` を `[u8; 4]` から `[u32; 4]` (Uint32x4 属性) に拡張し、256+ 関節の humanoid モデルで指 / 手首のスキニングが `skin_matrices[255]` に張り付かないようにした (#5); `load_primitive_morph_targets` が上限超過入力に対して warn + `.take(MAX_MORPH_TARGETS_PER_PRIMITIVE)` で切り詰めるようにした (#6); `ExpressionLayer::set_expression` / `apply_weights` が未知の表情名を拒否し、重みマップに格納しないようにした (#7); `load_vrm` が skin なしで `JOINTS_0` を持つモデルに対して警告を出すようにした (#8; レンダラーが沈黙の identity フォールバックを行っていたケース); `Runtime::RedrawRequested` の `head_world = character_position + (0, 1, 0)` のマジック ナンバーを `look_at::HEAD_OFFSET_Y` + `head_world_for(pivot)` に切り出した (#9); `const _: ()` ブロックでホスト側 `morph_offsets` 要素を 16 バイトに固定し、`[f32; 3]` への退行が WGSL `array<vec3<f32>>` ストライドの silent 不整合でなくビルド エラーになるようにした (#17)。`MAX_MORPH_TARGETS_PER_PRIMITIVE` はクレート ルートから再エクスポート済み。**PR4.7 humanoid bone registry** は `crates/ene-vrm/src/humanoid.rs` (~480 LoC, 7 件の unit test) を新設し、`VRMC_vrm.humanoid.humanBones` をパースして `HumanoidBoneRegistry` (bone 名 → `{ glTF node, 任意の Skeleton joint, rest translation + rotation }`) を提供する。55 骨のセットは VRM 1.0 仕様 canonical 名 (Hips..RightToes) で、canonical 名は lower-case;`canonicalize_bone_name` が mixed-case / snake_case / kebab-case / PascalCase を受け入れる。ヘルパ アクセサ (`head()` / `hips()` / `left_eye()` / `right_eye()` / `jaw()`) で、後続コンシューマ (#11 LookAt、#13 SpringBone、#14 VRMA、#15 NodeConstraint) の配線が単純になる。未知の bone 名は `tracing::warn!` で drop。registry は `VrmModel` に attach され、クレート ルートから再エクスポート済み。**per-frame のコンシューマは PR4.7 では追加しない** — registry 構築と rest transform 捕捉のみ。 |
| **PR5 — v2: クリックスルー + オフスクリーン マスク** | §4 PR5 | **PR5.2 出荷済み (Windows, per-bone sphere / capsule / Y-capsule コリダーに再々設計)**、PR5.3+ 未着手 | PR5.1 の BVH-trimesh 方式は rest pose 固定で per-frame アニメーションに追従できず、デフォルト VRMA 再生中に顔・足先がクリックできない問題があった。PR5.2 で per-bone コリダー方式に変更: `CharacterRenderer::build_character_bone_specs` が `model.humanoid` を walk して各 humanoid bone の `BoneShapeSpec` を発行。`BoneShape::Sphere` は頭 / 顎 / 目 / 肩 / つま先 (bone のレスト位置から実メッシュへの最大距離でサイジング)、`BoneShape::Capsule` は upper / lower arm + leg + hand (親 → bone レスト方向に沿って置き、腕のワイヤーフレームが limb 軸に沿うように)、`BoneShape::CapsuleY` は首 / 背骨 / 胸 / 上胸 / 尻 / 足 (trunk bone のローカル +Y、足は bone の rest world rotation に追従)。半径は実メッシュから — `VrmPrimitive` が `Vec<MeshVertex>` を保持し (load 時に clone)、`collider::collect_weighted_world_positions_into` が「支配的スキン重みが対象 bone に対する値で `VERTEX_WEIGHT_THRESHOLD = 0.25` 未満」の頂点を除外する。`PhysicsWorld::add_character_bone_colliders` が world 原点の単一 kinematic body にぶら下げ、`update_character_bone_positions` が `current_bone_poses` (`model.nodes.world_positions` を読んだもの *と* per-bone rest → current world rotation) → 各 collider の `set_translation_wrt_parent` *と* `set_rotation_wrt_parent` (axis-angle は `Quat::to_axis_angle`) でローカル更新。アニメーション追従 (位置と姿勢両方)、per-frame GPU readback ゼロ。`apps/ene-desktop-v2/src/platform/` モジュールは PR5.1 で削除済み (Wayland 実装時に復活)。 |
| **PR5.6 — レイキャスト コライダー デバッグ オーバーレイ (F3 + 設定チェックボックス)** | §4 PR5.6 | **出荷済み** | 新規 `ene_vrm::DebugRenderer` (line-list、3D 深度テストあり) が PR5.2 の bone collider ごとにワイヤーフレーム球 (アイドル時シアン、ヒット時黄) と、raycast ヒット点に 3 軸クロス (赤) を描画。パイプラインは `Depth32Float`、`depth_compare = Less`、`depth_write = false`、premultiplied alpha ブレンディング、色と深度の両方に `LoadOp::Load` を使い、モデル シルエットがワイヤを正しく隠れるようにしている。`AppState` が最新の `RaycastHit` (entity / toi / ワールド空間点 / collider handle) を保持し、ランタイムがヒットした collider をハイライト + ヒット点クロスを描画。オーバーレイはキャラクター ウィンドウの **F3** または Character 設定ページの新規「Raycast Colliders (Debug)」チェックボックスでトグル。状態は **永続化されない** — 起動ごとに OFF。 |
| **PR5.7 — `about_to_wait` 直接レンダリングで winit 0.30.13 マルチウィンドウ `RedrawRequested` バグを迂回** | §4 PR5.7 | **出荷済み** | キャラクター ウィンドウの render は `Runtime::about_to_wait` から直接駆動されるようになった (新規 `render_char_frame` メソッド)、`Window::request_redraw` → `Event::WindowEvent::RedrawRequested` → render パスを完全に迂回。UI ウィンドウは既に同じ理由で `about_to_wait` から render していたので、char ウィンドウもそのパターンに揃えた。winit 0.30.13 のバグ (`ControlFlow::WaitUntil(deadline)` + 2 ウィンドウ → プロセス内で最初に作成されたウィンドウに `RedrawRequested` が永遠に届かない; `apps/winit-waituntil-bug-repro` で確認) は、char レンダーが `RedrawRequested` 配送に依存しなくなったので無関係に。char レンダーが `RedrawRequested` 配送から切り離されたので、`WaitUntil(Instant + frame_interval)` フレーム ペーサーを復帰: `15/30/60/120 FPS` は `WaitUntil(deadline)`、`0` (Unlimited) は `Poll`。デッドラインは `last_frame_instant` (`render_char_frame` 冒頭で更新) を基準にし、コールドスタート用に `now()` フォールバック。これにより `update_motion` で使う `dt_secs` が選択レートと一致する。`resumed` の `set_control_flow(Poll)` はワンショット コールドスタート デフォルトとして保持。`handle_char_window_event` の `RedrawRequested` 腕は no-op (可読性 / 将来の再有効化のため残置)。`Runtime::char_surface_fatal` フラグを `AcquireError::Fatal` 時にセットし、次の `about_to_wait` 末尾でイベント ループを exit (旧 inline `event_loop.exit()` の代わり)。`apps/winit-waituntil-bug-repro` は永続的なリグレッション テスト + upstream 報告アーティファクトとして同梱 (`apps/winit-waituntil-bug-repro/WINIT-ISSUE.md` 参照)。 |
| **PR5.5 — v2: レガシー `apps/ene-desktop` (Bevy) のリネーム / 削除** | §4 PR5.5 | **未着手** | — |

### 0.1 2 つのデスクトップアプリの共存

PR1 の「スケルトン差し替え」ステップ 3 が完了するまでの間、両バイナリが並行してビルドされる:

- **`apps/ene-desktop`** — レガシー Bevy 0.18 ビルド。依然としてユーザー向けのデスクトップアプリ。依然として `bevy`, `bevy_egui`, `bevy_vrm1`, `tray-icon`, `gtk` (Linux), `wayland-client` (Linux) に依存。ローカルの `patches/bevy_winit` パッチは削除され、レガシービルドは crates.io の upstream `bevy_winit` 0.18 を使用するようになりました。**レガシーバイナリは本移行作業では一切変更されず**、リネームステップまでそのまま残ります。
- **`apps/ene-desktop-v2`** — 新設クレート、既存のものと並行に配置。`winit` + `wgpu` 29 + `egui` 0.34 の透過ウィンドウ煙幕 (PR0, §22.3) として開始。PR1 で tokio ランタイム駆動の `AppState`、`EneHandle` ラッパー、システムトレイ、および `CharacterSettings` 移植を追加。起動するには **`cargo run -p ene-desktop-v2`** を実行します。

当初の計画では「PR1 が削除ステップ」であり、`apps/ene-desktop` から Bevy を取り除き、そこに v2 のソースを移動することになっていました。しかし、この計画は**以下の新しいポリシーによって置き換えられ**、一方のバイナリで機能が動き、もう一方では動かないといった中途半半な破損状態を回避します。

#### 新しいポリシー (PR1 以降)

> **v2 は、レガシーの `apps/ene-desktop` (Bevy 0.18) の完全な代替品になるまで、PR1〜PR5+ を通じて段階的に拡張されます。レガシーのクレートは移行中もビルド可能な状態を維持します。`apps/ene-desktop-v2/` から `apps/ene-desktop/` へのリネームと、レガシー Bevy ソースの削除は、PR5 の最後 (PR5.5) に単一のコミットで一括して実行されます (条件として、§0 の PR1〜PR5 がすべて「出荷済み」になっている必要があります)。**

詳細な PR ロードマップは §4 に記載されています。PR5.5 に達するまでは、これら2つのクレートを**並行するコードベース**として扱います。

### 0.2 レシピの実証場所

PR0 が出荷した透過レシピは、ワークスペースに依然として残っている `apps/tw-test` (独立した Bevy 0.18 テストベッド) と同じものである。相互参照は §22.3 末尾に記載。ファイル自体は PR1 ステップ 3 でクリーンアップされるまで残置。

---

## 1. 背景と動機

現在の `ene-desktop` は Bevy 0.18 (`bevy_winit`, `bevy_egui`, `bevy_vrm1`, …) の上に構築されている。2 件の具体的なバグにより、Windows で本番ビルドが使い物にならない。

| # | バグ | トリガー | 影響 |
|---|------|---------|------|
| B1 | egui のレンダリングがクラッシュする | Windows + DX12 + `WGPU_DX12_PRESENTATION_SYSTEM=DxgiFromVisual` | egui がレンダリング中にパニックまたはクラッシュする。現状は環境変数を未設定にしてウィンドウ透過がピクセルパーフェクトでないことを許容するワークアラウンド。 |
| B2 | ウィンドウ透過が壊れる | Windows + Vulkan バックエンド | `transparent: true` を指定してもキャラクターウィンドウが完全不透明になり、デスクトップオーバーレイ機能全体が破綻。 |

加えて、Bevy ラッパー内の winit 問題を回避するため、ローカルに `bevy_winit` のパッチ (`patches/bevy_winit`、`[patch.crates-io]` で配線) を保持している。これは技術的負債であり、常にマージの苦痛を伴う。

### 1.1 なぜ Bevy を「修正」せず「置き換える」のか

- 2 件のバグは **Bevy の `Window` と wgpu サーフェスの境界** にあり、我々のコードの中にはない。Bevy 自体をフォークしない限り修正できない。
- 我々が使う Bevy は薄いスライスだけ: ウィンドウ、`Camera3d`、レンダーレイヤ分割、アセットサーバ、egui ブリッジ、そしてモデル用の `bevy_vrm1`。他の大部分 (システムトレイ、ホットキー、設定永続化、AI ブリッジ) は既に Bevy の外で手作りされている。
- `bevy_vrm1` 自体はアップストリームの `gltf` クレートを使い、MToon マテリアルを WGSL で適用している。このシェーダロジックと glTF 解析コードは `bevy_pbr` 依存を外せば再利用できる。
- 直接スタックはバイナリサイズで 5〜10 倍小さく、`bevy_ecs` マクロ経由の間接依存がゼロとなり、`bevy_winit` のフォーク保守からも開放される。

### 1.2 なぜ `ene-vrm` を別クレートにするのか

- VRM コード (glTF 解析、MToon シェーダ、表情 / LookAt、将来的な Spring Bone) は非自明で独自のテストを持つ。`ene-desktop` の中に置くと描画とアプリ配線が再び絡む。
- 独立クレート化により、テスト・ドキュメント・(将来的な) `ene-cli` のヘッドレスレンダリングやスクリーンショットテストでの再利用がクリーンになる。
- wgpu 27 のシェーダバックエンド切替を、デスクトップバイナリに触らずに行える。

---

## 2. ゴールと非ゴール

### 2.1 ゴール (必ず出す)

- G1. Windows 10/11 (DX12): `WGPU_DX12_PRESENTATION_SYSTEM=DxgiFromVisual` を設定した状態で、egui と 3D が透過ウィンドウに正しく合成される。
- G2. Linux + X11 (Vulkan): キャラクターウィンドウは透過、設定ウィンドウは不透明。
- G3. Linux + Wayland (Vulkan、利用可能なら layer-shell): キャラクターウィンドウは透過、コンポジタblur/クリッピングが正しく機能する適切なレイヤを使用。
- G4. `crates/ene-vrm` が glTF / VRM 1.0 を読み込み、スキニング行列を計算し、MToon で描画し、`LookAt` / 表情 API を提供する。
- G5. システムトレイ、ホットキー、設定永続化、AI ブリッジ (`ene-core` の `EneHandle`) はユーザ視点で変化せず動作。
- G6. 既存公開設定スキーマ (`assets/character_settings.schema.json` 等) は不変。

### 2.2 非ゴール (明示的に先送り)

- N1. VRMA 再生と Spring Bone シミュレーション。PR5+ で別管理。
- N2. フレーム単位のシャドウ品質切替 (FXAA / SMAA / TAA トグル)。当面は単一のデフォルトに固定。
- N3. VRM クレートの新規 C-ABI / プラグイン公開面。
- N4. macOS 対応。新コードは macOS でも Vulkan を使ってコンパイル可能だが、日常テストは対象外。
- N5. クロスプラットフォーム統一「透過抽象化」ライブラリ。プラットフォーム固有経路を受け入れる。

### 2.3 スコープ外の整理 (適宜実施)

- 不要になった `bevy_winit` パッチ (`patches/`) の削除。
- ワークスペース依存グラフから `bevy`, `bevy_pbr`, `bevy_winit`, `bevy_egui`, `bevy_vrm1` の削除。

---

## 3. 全体アーキテクチャ

```
┌──────────────────────────────────────────────────────────────────────┐
│                       apps/ene-desktop                              │
│  ┌──────────────────┐  ┌─────────────────────┐  ┌──────────────┐  │
│  │ main.rs          │  │ runtime/            │  │ tray.rs      │  │
│  │ (イベントループ,  │  │  EventLoop ポンプ    │  │ システムトレイ│  │
│  │  起動)            │  │  WindowSlot 管理     │  └──────────────┘  │
│  └────────┬─────────┘  └──────────┬──────────┘                     │
│           │                       │                                 │
│  ┌────────▼─────────┐  ┌──────────▼──────────┐                     │
│  │ gpu/             │  │ ui/                 │                     │
│  │  wgpu device,    │  │  egui コンテキスト,  │                     │
│  │  depth, camera   │  │  設定ウィンドウ     │                     │
│  └────────┬─────────┘  └──────────┬──────────┘                     │
│           │                       │                                 │
│  ┌────────▼─────────┐  ┌──────────▼──────────┐  ┌──────────────┐   │
│  │ character/       │  │ ai_bridge.rs        │  │ platform/    │   │
│  │  ドラッグ,        │  │  EneHandle ポーリング│  │  HWND,       │   │
│  │  VrmHandle 接着   │  │  VecDeque<EneEvent> │  │  Wayland lyr │   │
│  └────────┬─────────┘  └─────────────────────┘  └──────────────┘   │
│           │                                                          │
└───────────┼──────────────────────────────────────────────────────────┘
            │ uses
            ▼
┌──────────────────────────────────────────────────────────────────────┐
│                       crates/ene-vrm                                │
│  loader (gltf) → model (VrmModel) → skeleton → mtoon (WGSL)        │
│  → renderer (wgpu::Surface に描画)                                  │
│  expression, look_at, (後) spring_bone, vrma                        │
└──────────────────────────────────────────────────────────────────────┘
            │ uses
            ▼
     wgpu 27, winit 0.30, egui 0.33 (egui-wgpu, egui-winit)
     gltf 1.4, glam 0.29, encase 0.12, bytemuck, pollster
```

### 3.1 モジュール境界

- `runtime/` は `winit::EventLoop`、`wgpu::Instance / Device / Queue`、`HashMap<WindowId, WindowSlot>` を所有する。このレイヤは VRM モデルや設定を知らない。
- `gpu/` は wgpu デバイスと深度バッファを作る小さな汎用ヘルパ。アプリ固有状態を持たない。
- `ui/` はウィンドウ単位の egui 統合。各 `WindowSlot` は自身の `egui::Context`, `egui_winit::State`, `egui_wgpu::Renderer` を所有する。
- `character/` は `ene_vrm::VrmRenderer` を消費し、`WindowSlot` にバインドする。
- `ene-vrm` はプラットフォーム非依存。`wgpu::Device`, `wgpu::RenderPass`, `CameraUniform` を受け取り描画する。winit を知らない。

---

## 4. 段階的 PR 計画

小さく個別レビュー可能な PR の連なりとして移行を進める。各 PR は `cargo build --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` をグリーンに保つこと。**AGENTS.md §10 に従い、開発初期フェーズでは `main` への直接コミットも許容する。**

### PR1 — スケルトン差し替え (最も難しい PR)

> **ステータス:** 進行中。 **ステップ 2** (`crates/ene-vrm` スケルトン) とレシピの動機となった **PR0 透過スモーク** は出荷済み; PR1 の残り (ステップ 3〜8: 既存 `apps/ene-desktop` から Bevy を剥がし、ソースを v2 へ移し、winit ウィンドウ・トレイ・AI ブリッジ・設定を移植) は未着手。現状は §0 と §22.3 を参照。

**目的:** Bevy を除去し、クリアカラーだけの透過ウィンドウと動作するシステムトレイを実現。AI ブリッジはフックされたままで、描画は何もしない。

**手順**

1. ワークスペース `Cargo.toml` を編集:
   - `[workspace.dependencies]` に `wgpu = "27"`, `winit = "0.30"`, `egui = "0.33"`, `egui-wgpu = "0.33"`, `egui-winit = "0.33"`, `glam = "0.29"`, `gltf = "1.4"`, `encase = "0.12"`, `bytemuck = "1"`, `pollster = "0.4"`, `raw-window-handle = "0.6"` を追加。
   - `[workspace.dependencies]` および `apps/ene-desktop/Cargo.toml` から `bevy`, `bevy_pbr`, `bevy_winit`, `bevy_egui`, `bevy_vrm1`, `bevy_animation`, `bevy_asset`, `bevy_render`, `bevy_math`, `bevy_mesh`, `bevy_window`, `bevy_input`, `bevy_image`, `bevy_transform`, `bevy_utils`, `bevy_ecs` を削除。
   - `[patch.crates-io] bevy_winit = { path = "patches/bevy_winit" }` を削除し、`patches/bevy_winit` ディレクトリを削除。
2. `crates/ene-vrm/` を `Cargo.toml` と `src/lib.rs` (`pub fn version() -> &'static str` のみ) で作成し、ワークスペースメンバに追加 (`crates/*` で既にカバーされている)。
3. `apps/ene-desktop/Cargo.toml` の依存をワークスペースのものに書き換え。
4. `apps/ene-desktop/src/main.rs` を書き換え:
   - `tokio::runtime::Runtime::new()` と `runtime.enter()` は維持。
    - `wgpu::Instance` (Windows は DX12 + `DxgiFromVisual`、Linux / macOS は Vulkan) と `wgpu::Adapter` + `wgpu::Device` + `wgpu::Queue` を `pollster::block_on` もしくは oneshot で初期化。
   - `winit::EventLoop` を構築し、`ControlFlow::Wait` を設定し、`window_plugin()` を反映する `WindowAttributes` を持つプライマリキャラクターウィンドウ (`Arc<winit::Window>`) を登録: `WindowLevel::AlwaysOnTop`, `transparent: true`, `decorations: false`, `resizable: true`, `inner_size: (320, 480)`。
    - Windows では `Instance::new` の前に `std::env::set_var("WGPU_DX12_PRESENTATION_SYSTEM", "DxgiFromVisual")` を設定し、DX12 バックエンドを使用。
   - Linux では早期に `gtk::init()` を呼ぶ (トレイ用)。
   - フレーム毎ハンドラ: プライマリウィンドウを `clear_color: (0,0,0,0)` で再描画し、present して戻る。
5. AI ブリッジを移植: `bevy::Message` を捨て、`ai_bridge::AiBridge { handle: EneHandle, receiver, processing: AtomicBool, pending: Mutex<VecDeque<EneStreamEvent>> }` を導入。ブリッジは `EneHandle::events()` から pull して deque に push するバックグラウンド tokio タスクを起動する。
6. トレイを移植: `tray-icon` をそのまま維持。Windows のメッセージスレッドは winit から独立。Linux では GTK メインスレッドがトレイをポーリングする。winit イベントループがメインスレッドで作成されることに注意 (される、`gtk::init` は main を奪わない)。
7. 設定ウィンドウの移植: **PR2 に先送り**。当面のトーストは "unimplemented" でも可。
8. **検証:** 手動スモークのみ。`cargo run -p ene-desktop` で透明 (もしくは真っ白) なウィンドウがカーソルに追随し、リサイズで再生成されれば OK。VRM も設定もまだ無い。

**変更 / 新規ファイル**

- `Cargo.toml` (ワークスペース): 依存変更
- `apps/ene-desktop/Cargo.toml`: 依存変更
- `apps/ene-desktop/src/main.rs`: 書き換え
- `apps/ene-desktop/src/{app_config,resources,ai_bridge,tray,platform}.rs`: 書き換え (Bevy を除去、公開関数を維持)
- `apps/ene-desktop/src/{scene,character,settings_ui,character_drag}.rs`: **削除** (PR1 が削除ステップ。後続 PR で移植)
- `crates/ene-vrm/Cargo.toml` + `src/lib.rs`: 新規
- `patches/bevy_winit/`: 削除

### PR2 — egui 統合 + 設定ウィンドウ

**目的:** egui をウィンドウ毎レンダラに配線し、設定ウィンドウを復帰させる。

**手順**

1. 新モジュール `apps/ene-desktop/src/runtime/window_slot.rs`:
   ```rust
   pub struct WindowSlot {
       pub window: Arc<winit::Window>,
       pub surface: wgpu::Surface<'static>,
       pub config: wgpu::SurfaceConfiguration,
       pub depth: wgpu::Texture,           // PR3+ 向け、遅延生成
       pub egui_ctx: egui::Context,
       pub egui_state: egui_winit::State,
       pub egui_renderer: egui_wgpu::Renderer,
   }
   ```
2. 新モジュール `apps/ene-desktop/src/runtime/surface.rs`: `Resized`, `ScaleFactorChanged` 時に `wgpu::Surface` を生成 / 再構成。
3. 新モジュール `apps/ene-desktop/src/runtime/input.rs`: `winit::WindowEvent` を `egui_state.on_window_event` (egui コンテキストを持つウィンドウ) またはアプリレベルハンドラ (ドラッグ、ホットキー) に振り分け。
4. ウィンドウ毎フレームのレンダリングパイプライン:
   1. 入力から `egui_ctx` を更新。
   2. `ui::paint(window_id, &egui_ctx, &state.settings, &state.ai)` クロージャを実行し `egui::FullOutput` を取得。
   3. `egui_ctx.tessellate(...)` → メッシュ群。
   4. wgpu コマンドエンコーダ符号化: キャラクターウィンドウは `(0,0,0,0)` でクリア、設定ウィンドウは不透明色でクリア。3D はまだ描かない。最後に `egui_renderer.render(...)` をアルファブレンドで合成。
5. 新モジュール `apps/ene-desktop/src/ui/`:
   - `mod.rs` — `pub fn paint(ctx: &egui::Context, settings: &mut CharacterSettings, ai: &mut AiBridge)`。表示ページを決定。
   - `page_ai.rs`, `page_character.rs`, `page_graphics.rs`, `widgets.rs` — `settings_ui/` から移植。
6. トレイメニューに "Settings" を追加し、独自 `WindowSlot` と egui コンテキストを持つ 2 つ目の `winit::Window` (不透明) を開く。

**検証**

- Windows で `DxgiFromVisual` 下、egui パネルが両ウィンドウで正しい位置に表示。
- Linux X11 でキャラクターウィンドウが透過、設定ウィンドウが不透明。
- 設定ウィンドウをタイトルバー閉じで `WindowSlot` を破棄しサーフェスを解放。
- `cargo clippy --workspace -- -D warnings` クリーン。

### PR3 — `ene-vrm` 静的描画 (MToon + スキニング)

**目的:** `.vrm` を読み込み、手書きの MToon WGSL シェーダで描画する。

**サブタスク**

1. **`crates/ene-vrm/src/loader.rs`** — `gltf` クレートで `.vrm` (glTF バイナリに `extensionsUsed: ["VRMC_vrm", …]` を含む) を読む。抽出:
   - スキン、逆バインド行列、ジョイント。
   - メッシュ + プリミティブの頂点 / インデックスバッファ。
   - 各プリミティブマテリアルの `KHR_materials_unlit` 拡張から MToon マテリアルパラメータ。
   - テクスチャデータ (base color, normal, emission, shade, matcap, rim) — `Vec<Image>` として読み、`wgpu::Queue::write_texture` でアップロード。
2. **`crates/ene-vrm/src/model.rs`** — 公開型:
   ```rust
   pub struct VrmModel {
       pub meshes: Vec<MeshGpu>,
       pub skeleton: Skeleton,
       pub materials: Vec<MToonMaterial>,
       pub textures: Vec<wgpu::Texture>,
       pub nodes: Vec<Node>,
       pub root: NodeIndex,
   }
   pub struct Skeleton { pub joints: Vec<Joint>, pub inverse_bind: Vec<Mat4> }
   pub struct Joint { pub node: NodeIndex, pub local_bind: Transform }
   ```
3. **`crates/ene-vrm/src/skeleton.rs`** — ノード階層と任意の `AnimationSampler` から現在のスキニング行列 `&[Mat4]` を計算 (PR5+ までアニメは単位行列)。
4. **`crates/ene-vrm/src/mtoon.rs`** + `shaders/mtoon.wgsl`:
   - 描画毎ユニフォーム: `MToonUniform { base_color, shade_color, shading_shift, … }`。
   - バインドグループ: `(0)` スキニングストレージバッファ、`(1)` マテリアル UBO、`(2)` フレーム毎カメラ。
   - フラグメントは MToon ライティングモデル (lit / shade / rim / matcap / outline) を実装。アウトラインは別レンダーパスで `cull_mode: Front`、法線方向に頂点を膨張。
5. **`crates/ene-vrm/src/renderer.rs`**:
   ```rust
   pub struct VrmRenderer {
       pipeline: wgpu::RenderPipeline,
       outline_pipeline: wgpu::RenderPipeline,
       skin_buf: wgpu::Buffer,
       camera_buf: wgpu::Buffer,
       camera_bgl: wgpu::BindGroupLayout,
   }
   impl VrmRenderer {
       pub fn new(device, surface_format) -> Self;
       pub fn render(
           &self,
           encoder: &mut wgpu::CommandEncoder,
           view: &wgpu::TextureView,
           depth: &wgpu::TextureView,
           model: &VrmModel,
           camera: &CameraUniform,
       );
   }
   ```
6. **`apps/ene-desktop/src/character/mod.rs`** — 既定 VRM を読み込み `WindowSlot::vrm_model` に格納、`VrmRenderer::render` をクリアと egui パスの間で駆動。
7. フレーム毎の描画順: クリア → アウトライン → メイン → egui。

**検証**

- 既知の `.vrm` (同梱サンプル等) を投入し、カメラ前方に立っているのが見える。
- ウィンドウをドラッグするとカメラ距離が更新され、モデルは中央に留まる。
- `cargo test -p ene-vrm` (ローダとスケルトン数学の新規ユニットテスト)。

### PR4 — 表情、LookAt、BodyTracking

> **ステータス:** **Shipped** (2026-06-18、コミット待ち)。**PR4.1 (ModelUniform + カリング)**、**PR4.2 (LookAt カーソル射影 + BodyTracking プロファイル)**、**PR4.3 (ドラッグで移動)**、**PR4.4 (表情 / モーフターゲット)**、**PR4.5 (スキニング — rest-pose パレット)**、**PR4.6 (quick-win ハードニング)**、**PR4.7 (humanoid bone registry)**、**PR4.8 (`VRMC_vrm.lookAt` パース + 毎フレーム評価器)**、**PR4.9 (表情オーバーライド + isBinary)**、**PR4.10 (アルファモードソート + 2 パス描画)**、**PR4.11 (`KHR_materials_unlit` 別パイプライン)** (issue #19)、**PR4.12 (`VRMC_node_constraint` パース + 評価器)** (issue #15)、**PR4.13 (`VRMC_springBone` パース + verlet シミュレータ)** (issue #13)、**PR4.14 (`VRMC_vrm_animation` (VRMA) パース + 再生エンジン)** (issue #14)、**PR4.15 (フル `VRMC_materials_mtoon` パース + per-material uniform + MToon シェーダパイプライン)** (issue #12)、**PR4.16 (評価器からの per-joint 骨回転 + skin-palette アップロード)** — PR4 クローズ。PR4 のステップ一覧 (表情、LookAt、BodyTracking) はレガシー計画から変更なしだが、消費側は `apps/ene-desktop-v2::character` (新規) / `runtime` (既存) に移動し、新しくドラッグで移動ステップ (旧レガシー `character_drag/mod.rs` プラグイン) の初版がここに入る。フル クリックスルー (Win32 `WM_NCHITTEST`、Wayland `wl_surface::set_input_region`、X11 shape 拡張、オフスクリーン マスク キャプチャ) は PR5。

**PR4.4 進捗 (本 PR で出荷済み)**

- **データモデル** (`crates/ene-vrm/src/expression.rs`):
  - `ExpressionName` newtype (大文字小文字は保持、VRM 1.0 規約で小文字正規化)。
  - `PrimitiveId`、`MorphTarget { name, position_offsets: Vec<[f32; 3]> }`、`PrimitiveMorphs { primitive_id, targets, name_to_slot: BTreeMap<ExpressionName, u32>, target_count, vertex_count }` — モーフターゲットを持つプリミティブごとに 1 個。
  - `ExpressionLayer { per_primitive: Vec<Option<PrimitiveMorphs>>, weights: BTreeMap<ExpressionName, f32> }` を `VrmModel` に保持。
  - `PrimitiveMorphMeta` ユニフォーム (`#[repr(C)] Pod`): `vertex_count: u32`、`target_count: u32`、2 個の `u32` パッド、`[[f32; 4]; 16]` パック済み重み (= プリミティブあたり最大 64 スロット)。WGSL `MorphMeta` 構造体と 1 バイト単位で一致。
- **ローダ** (`crates/ene-vrm/src/loader.rs`):
  - `resolve_expression_names(gltf)` が `Document::extensions()["VRMC_vrm"]["expressions"].{preset,custom}.<name>.morphTargetBinds[*]` を走査し、各 `{node, index}` を `Node::mesh().index()` 経由で `(mesh_idx, prim_idx, morph_target_index)` に解決する。そのメッシュの **全** プリミティブに名前をバインド (仕様では「全プリミティブが同じ morphTarget を共有する」)。エクステンションが参照しないターゲットは `morph_target_<i>` にフォールバック。
- **レンダラ** (`crates/ene-vrm/src/renderer.rs`):
  - 新規バインドグループレイアウト `(3)` — モーフ用 `storage<read>` (`morph_offsets: array<vec3<f32>>`、長さ `target_count * vertex_count`) と `uniform` (`morph_meta`、`min_binding_size = PrimitiveMorphMeta::SIZE`)。
  - モーフを持つプリミティブごとに `MorphGpu { offsets_buf, meta_buf, bind_group }` を 1 個、モーフを持たないプリミティブは単一の `DummyMorphGpu` を共有 (シェーダの `target_count == 0u` 早期脱出で参照スキップ)。
- **シェーダ** (`crates/ene-vrm/src/shaders/mtoon_lite.wgsl`):
  - `struct MorphMeta { vertex_count, target_count, _pad0, _pad1, weights: array<vec4<f32>, 16> }`。
  - `@group(3) @binding(0) var<storage, read> morph_offsets: array<vec3<f32>>`、`@group(3) @binding(1) var<uniform> morph_meta: MorphMeta`。
  - `vs_main` は `@builtin(vertex_index) vidx: u32` を取り、`morph_offsets[t * vertex_count + vidx] * weights[t/4][t%4]` を `morph_delta: vec3<f32>` に累積し、`view_proj * world_pos` の前に `world_pos` に加算。
- **ランタイム** (`apps/ene-desktop-v2/src/character_state.rs`、`character.rs`、`runtime.rs`):
  - `EmotionCommand` は `weight: f32` (AI ブリッジと手動ボタンの両方とも既定 1.0) を運ぶようになった。`EmotionQueue::drain_due(now_secs)` が期日到来コマンドと未来スケジュール (リップシンク用プレースホルダ) を分離する。
  - `ActiveEmotion { name, weight, hold_until_secs }` がレンダラの「現在表示中」の表情。
  - `CharacterRenderer::apply_emotions(&mut EmotionQueue, now_secs)` を `Runtime::about_to_wait` から 1 フレーム 1 回 (EmoteToken drain ループの直後、再描画前) 呼び出す。期日到来コマンドを drain し、`VrmModel::expressions_mut().set_expression(&ExpressionName::from(name.as_str()), weight)` を呼び、`hold_secs` 経過後にアクティブ表情の重みを `FADE_RATE = 0.9` で乗算フェードする (重みが `FADE_FLOOR = 0.01` を下回ったら `None` にする)。
  - **重みクリアの不変条件:** 表情 → 重みパイプラインは *マージ* ではなく *置換* である。`happy` → `neutral` (同梱の Alicia モデルではモーフ ターゲットではない) に切り替えた場合、前の `happy` の重みをゼロクリアしないとシェーダはまぶたを細め続ける。純粋ヘルパ `transition_emotions(drained, current, now_secs, fade_rate, fade_floor) -> (Option<ActiveEmotion>, Vec<(String, f32)>)` を `character_state::transition_emotions` に切り出し、新しいコマンドが異なる名前で来たときに `(prev.name, 0.0)` を `(new.name, weight)` の *前* に emit することで、レンダラの `set_expression` が先にクリアを適用するようにしている。これはレガシー Bevy アプリの `SetExpressions` リソース (毎フレーム存在しない名前を暗黙にドロップしていた) と同じ振る舞い。

**PR4.3 進捗 (本 PR で出荷済み)**

- 新 `apps/ene-desktop-v2/src/character/` フォルダ (旧 `character.rs` を置き換え):
  - `mod.rs` — `CharacterRenderer` に `pub drag: CharacterDragState` フィールドと `aabb_world(&ModelUniform)` アクセサを追加。
  - `drag.rs` — レガシー `apps/ene-desktop/src/character_drag/mod.rs` の v2 スタック向け 1:1 移植。
- ステート マシン (`apps/ene-desktop-v2/src/character/drag.rs`):
  - `CharacterDragState { last_cursor_world_pos: Option<Vec2> }` + `is_dragging()` (後者は PR5.1 でクリックスルー `allows_input` に接続するまで `#[allow(dead_code)]`)。
  - `enum DragButtonEvent { Pressed, Released }` + `enum DragAction { None, Started, Ended }` (`tick` ヘルパは `Option<Vec3>` を直接返すので、`DragAction` は press/release 専用に絞った)。
  - `on_press_or_release(state, event, cursor_world_2d, cursor_over_character) -> DragAction`: キャラクター上で押下 → ドラッグ開始 (ワールド座標カーソルを記憶)、リリース → ドラッグ終了。ドラッグしていない時のリリースは no-op。
  - `tick(state, cursor_world_2d) -> Option<Vec3>`: 1 フレーム デルタ `(new - last).extend(0.0)`、レガシー `update_drag_state` と同一の算術。
- 数学ヘルパ (すべてレガシーと 1:1):
  - `aabb_world_corners` (8 頂点変換) と `transformed_aabb_bounds` (ワールド AABB)。
  - `ray_intersects_aabb` (スラブ テスト、eps=1e-6、軸ごとのクロージャ、同一)。
  - `cursor_logical_to_world_2d` (NDC → view-z=0 平面 → ワールド 2D) — `look_at::compute_world_target` の `view_pos = Vec3::new(ndc.x * half_w, ndc.y * half_h, 0)` パターンを踏襲。ortho では絶対ワールド位置はカメラ eye になる; ドラッグ システムは 2 サンプル間の *差分* のみを必要とする (Bevy `Camera::viewport_to_world_2d` の意味論と同じ)。
  - `cursor_over_character` (毎フレーム ヒット テスト: カーソル レイ vs. 変形後ワールド AABB)。
- ランタイム配線 (`apps/ene-desktop-v2/src/runtime.rs`):
  - 新ヘルパ `cursor_world_2d_for_char_window(cw, position)` と `cursor_over_char_window(cw, character, settings, position)` で winit `PhysicalPosition` 入力に対する射影とヒット テストをラップ。
  - `WindowEvent::CursorMoved` が `character::drag::tick(&mut character.drag, cursor_world_2d)` を呼び、`None` 以外のデルタを `settings.character_state.character_position` に積分 (`cw` と `character` を独立に借用できるよう mut-borrow を分離)。
  - `WindowEvent::MouseInput { state, button: Left }` が `character::drag::on_press_or_release` を呼び、`DragAction::Ended` の時に `settings.mark_dirty()`。
  - 新インポート: `winit::event::MouseButton` (既存の `ElementState` に追加)。
- 12 件の新規ユニット テスト (`character::drag::tests`): AABB 変換 (平行移動 + スケール)、恒等変換 bounds、レイ ヒット + ミス (軸並行)、キャラクター上押下でドラッグ開始、キャラクター外押下は no-op、カーソル位置なし押下は no-op、ドラッグ中リリースで終了、未ドラッグ リリースは no-op、アイドル時 tick は `None`、不変時 tick は `None`、移動時 tick はデルタ返却 + 原点前進、カーソル消失時 tick は状態保持、ortho 中心がカメラ eye に射影、ワールド デルタがカーソル ピクセル デルタに比例、退化 viewport は `None`。
- **PR4.3 スコープ外:** クリックスルー/パススルー。ランタイムは winit ヒット テストをオーバーライドしないので、キャラクター ウィンドウ全面がまだクリック可能。PR5.1 (Windows: `SetWindowSubclass` + `WM_NCHITTEST` + `WS_EX_TRANSPARENT`) と PR5.2 (Wayland: `wl_surface::set_input_region`) がその作業を引き受ける。`is_dragging()` アクセサは `allows_input = cursor_over_character || drag_state.is_dragging()` 述語用に予約済み。

**PR4.16 進捗 (本 PR で出荷済み)** — PR4 クローズ

- `VrmModel::update_skin_palette` (`crates/ene-vrm/src/model.rs`) に第 2 引数 `look_at: Option<&LookAtBoneOutput>` を追加。`"bone"` 型モデルではランタイムが `CharacterRenderer::look_at_bone_output()` (`LookAtEvaluator` のスムージング済み出力) を渡し、`"expression"` 型モデルおよび `lookAt` 拡張を定義しない VRM では呼び出し側が `None` を渡すことで既存の rest-pose 数学は不変となる。シグネチャは後方互換で、テスト ヘルパ `model_palette(model, frame)` は新引数に単に `None` を渡すだけ。
- 合成は VRMA 骨回転と階層 walk の間に新しい **ステップ 2.5** として実装。`head` / `leftEye` / `rightEye` の各々について humanoid 骨を引き (humanoid registry がないモデル — VRM 0.x フォールバック — はおとなしく skip)、局所回転を `rest_local_rotation * look_at_delta` で上書きする。仕様では delta は現局所回転に対する差分ではなく骨の *rest* 回転に対する適用と定義されているため、motion 側でも回す頭 / 目は 2 つのソースをブレンドせず最終的にカーソル方向を向く。単位 delta は skip (rest pose を自身に回すだけになり、VRMA の結果を残す方が安価)。
- `apps/ene-desktop-v2/src/character/mod.rs::CharacterRenderer::update_motion` で wire-through (呼び出し 1 箇所、1 行): `model.update_skin_palette(&frame, self.look_at_bone_output.as_ref())`。診断用 `look_at_bone_output()` アクセサは `#[allow(dead_code)]` を維持するが (毎フレーム読み出しは `update_motion` 内)、rationale コメントは PR4.8 プレースホルダ テキストから "Consumed by `update_motion` (PR4.16)" に更新。
- `crates/ene-vrm/src/model.rs::tests` に新規 4 件のユニット テスト:
  - `update_skin_palette_applies_look_at_bone_delta_to_head` — 90° yaw LookAt が head 骨の +X 軸を -Z に回す。
  - `update_skin_palette_look_at_overrides_vrma_for_head` — 衝突する VRMA head 回転に対して LookAt が勝つ (rest-pose セマンティクス)。
  - `update_skin_palette_look_at_idempotent_for_zero_delta_or_missing_bones` — 単位 delta および欠落 humanoid エントリは VRMA 結果に手を加えない。
  - `update_skin_palette_look_at_ignores_missing_humanoid_bones` — humanoid registry なしモデル (旧 VRM 0.x) は panic ではなく no-op。
- **PR4.16 スコープ外:** 仕様にある spine → head → eyes チェーンの 2-bone IK。今回のカーソル target は head / eyes のみを回す; 肩と脊椎は触らない (レガシー `bevy_vrm1` の挙動と §0 description に一致)。`CharacterRenderer::body_tracking(strength)` がオンデマンド計算する `body_tracking` プロファイルは下流 PR (例: spring-bone の二次モーション) では依然有用だが、その重みはソルバーにはまだ接続されていない。

**PR-fix フォローアップ (マージ後、2026-06-18)** — ガクガクの look-at + 効かないドラッグ

PR4.16 ランドを実機で動かしたところ、毎フレームの座標空間取り違えに根を持つ 2 つの問題が浮上した。両方とも per-frame 配線の微妙な座標系ミス:

- **A1 — ガクガク look-at + カメラ低すぎ (初回 fix で退行、ここで訂正)。** 元の `update_camera_target` は `model.nodes.world_positions[chest.node][1]` (前フレーム VRMA 由来の *animated* chest 骨 world Y) を読み取っていた。chest を揺らすモーションが動くと camera target が揺れ、それに伴い cursor→world 射影も揺れ、head look-at が見た目ガクガクになっていた。初回 fix はこれを `chest.rest.translation[1]` に置き換えた — しかしこのフィールドは **local** glTF `Node::transform()` (親骨からのオフセット) であって、world 位置ではない。階層深くにある humanoid chest (hips → spine → chest → …) では local 値は親からのオフセット (~0.2 m) のみだが、world 値は全祖先の和 (~1.4 m) となる。初回 fix は camera target を約 7 倍低くし、フレーミング (頭頂部しか見えない)、look-at (cursor→world 射影が頭骨位置に一致しない)、ドラッグ (モデルが見えにくくクリックできない) を全て壊した。訂正済み fix は新規 private ヘルパ `chest_world_rest_y(model, bone) -> f32` で、`model.nodes.parents` + `model.nodes.rest_local_positions` 経由で親チェーンを辿り、各祖先の local Y を累積する (ローダーの `NodeHierarchy::compute_world_transforms` と同じ計算、単一骨の Y 軸のみ)。pure ヘルパ `chest_target_y(world_y, center_y, normalize_scale, model_scale) -> f32` は *world* rest Y を受け取るようになった。使われていなかった `character_position` 引数は削除 (カメラは world 固定; モデルは per-frame `model_matrix` 経由で view 内に位置決めされる)。`character::camera_target_tests` に 5 つのリグレッション テストを追加: pure ヘルパの手計算例、3 リンク親チェーン合計テスト (`chest_world_rest_y_sums_parent_chain_offsets`)、X / Z の親オフセットが world Y 合計に漏れないことを保証する同軸のみテスト (`chest_world_rest_y_sums_only_y_axis`)、`update_camera_target` 呼び出し間で `world_positions[1]` を mutation しても target Y が不変であることを確認する安定性テスト (`update_camera_target_is_stable_and_uses_world_rest_y`)、no-model 安全性テスト。
- **B1 — 効かないドラッグ。** `update_character_collider` (`apps/ene-desktop-v2/src/physics.rs`) は生 AABB と `scale` のみから Rapier cuboid を組み、`normalize_scale` を無視し、collider の中心を `translation` ではなく `translation + raw_center` に置いていた。Alicia (`normalize_scale ≈ 0.5`、AABB `([-1, 0, -1], [1, 2, 1])`) では collider が約 2 倍大きく、約 1 m 上にずれていた。結果、camera eye から cursor を通る Rapier raycast が silhouette に当たらず、`cursor_over_char_window` が常に `false` を返し、`on_press_or_release` が `DragAction::None` に短絡してドラッグが始まらなかった。修正は新パラメータ `normalize_scale: f32` を追加し、局所半延長を `(max - min) * 0.5 * normalize_scale * scale` にし、rigid body 中心を `translation` に置く (モデルの局所原点 = world の `translation`; `T(-center)` ステップが scale より *前* に mesh を原点中心に再配置するため、collider の中心はモデル自体と同じ点になる) こと。runtime 呼び出し側は `model.normalize_scale()` を渡すようになった。`physics::tests` に 2 つのリグレッション テストを追加: 単位立方体 AABB + `normalize_scale = 0.5` で原点中心の `0.5` half-extent cuboid が生成されること、非対称 AABB + y シフト translation で collider が `translation + raw_center` ではなく `translation` を中心とすること。

同じコミットに drive-by cleanup もある: `Runtime::last_cursor_logical` を `last_cursor_physical` にリネーム (フィールドは `PhysicalPosition<f64>` で、look-at が使う cursor パスは physical pixel として扱う — 内部一貫だが旧名は嘘)。別 struct の `LookAtState::last_cursor_logical: Option<Vec2>` (こちらは実際に logical pixel) は名称維持。

**検証 (PR4 全体)**

- OS カーソルを動かすとモデルの頭 / 目がクランプ角内で追従 (PR4.16 で評価器出力を `update_skin_palette` に通したため実際に回転が視覚化される; PR4.16 以前は `look_at_target()` アクセサが報告のみでスキニング数学には組み込まれていなかった)。
- チャットに "I'm so happy!" を打つと `happy` ブレンドシェイプへ遷移 (PR4.4 で有効化済み)。
- `ene-vrm` に自動テスト: 表情を設定し、`ExpressionLayer::set_expression` のクランプ / 名前引きを検証 (8 テスト合格済み)。PR4.16 合成 (4 テスト) も追加。
- 6 個の手動表情テスト ボタン (`SettingsUi::emotion_queue` を push) で VRM の表情変化を目視確認 (PR4.4 で出荷済み)。

### PR5 — クリックスルー + オフスクリーン マスク

> **ステータス (2026-06-20):** **PR5.2 (Windows) 出荷済み — per-bone Rapier コリダー方式 (sphere / capsule / Y-capsule。実メッシュの per-vertex スキン重みからサイジング、per-frame translation *と* rotation 更新で limb capsule がアニメーションに追従) に再々設計。** PR5.1 の BVH-trimesh 方式は rest pose 固定で per-frame アニメーションに追従できず、デフォルト VRMA 再生中に顔・足先がクリックできない問題があった。PR5.2 で `CharacterRenderer::build_character_bone_specs` が `model.humanoid` を walk して各 humanoid bone の `BoneShapeSpec` を発行。`BoneShape::Sphere` は頭 / 顎 / 目 / 肩 / つま先 (bone のレスト位置から実メッシュへの最大距離でサイジング)、`BoneShape::Capsule` は upper / lower arm + leg + hand (親 → bone レスト方向に沿って置き、腕のワイヤーフレームが世界の +Y ではなく limb 軸に沿うように)、`BoneShape::CapsuleY` は首 / 背骨 / 胸 / 上胸 / 尻 / 足 (trunk bone のローカル +Y、足は bone の rest world rotation に追従)。半径は実メッシュから計算 — `VrmPrimitive` が `Vec<MeshVertex>` を保持するようになり (load 時に clone)、`collider::collect_weighted_world_positions_into` が「支配的スキン重みが対象 bone に対する値で `VERTEX_WEIGHT_THRESHOLD = 0.25` 未満」の頂点を除外する (肩に紐づく頂点が腕 capsule を太らせない)。スキニング対象外 bone (inverse_bind なし) と `MIN_BONE_RADIUS = 0.025` 未満の小さい bone (指) はスキップ。`PhysicsWorld::add_character_bone_colliders` が world 原点に置いた単一 kinematic body にぶら下げ、`update_character_bone_positions` が `current_bone_poses` (これが `model.nodes.world_positions` を読み、`update_skin_palette` が毎フレーム更新し、*加えて* per-bone rest → current world rotation を読む) → 各 collider の `set_translation_wrt_parent` *と* `set_rotation_wrt_parent` (axis-angle は `Quat::to_axis_angle`) で位置 *と* 姿勢をローカル更新。アニメーションに追従 (位置と姿勢の両方)、per-frame GPU readback ゼロ。**PR5.6 (コライダー デバッグ オーバーレイ、今や capsule ワイヤーフレームも描画) および PR5.7 (`about_to_wait` 直接レンダリング — winit 0.30.13 マルチウィンドウ `RedrawRequested` バグを迂回し `WaitUntil(deadline)` ペーサーを復帰) 出荷済み。** PR5.3 以降 (Wayland input region、X11 shape、Linux オフスクリーン マスク + gizmo) は未着手のまま。

**目的:** キャラクター シルエットの外側がクリック / ホバー / フォーカスをデスクトップに透過する。背景のウィンドウ操作性を維持しつつ、キャラクターの操作性を保持する。

**サブタスク (Windows 先行、Linux は後続)**

1. **Windows クリックスルー (PR5.2 — 2026-06-19 出荷済み, 2 度目の再設計)** — 3 つのピース、Win32 を直接叩かない:
   - `Window::set_cursor_hittest(allows_input)` (winit)。Windows では `WS_EX_TRANSPARENT` + `WS_EX_LAYERED` をトグル + `SetWindowPos(SWP_FRAMECHANGED)` を発火、他 OS では no-op。
   - グローバルカーソルは `device_query` クレート (`device_query::DeviceState::new().get_mouse().coords`) で毎フレーム取得、ウィンドウローカル座標へは `Window::outer_position()` と scale factor で変換。
   - ヒットテストは per-bone Rapier コリダーに対する raycast。`CharacterRenderer::build_character_bone_specs` が `model.humanoid` を walk して各 humanoid bone の `BoneShapeSpec` を発行。`BoneShape::Sphere` (頭 / 顎 / 目 / 肩 / つま先) は bone のレスト位置から実メッシュへの最大距離でサイジング、`BoneShape::Capsule` (upper / lower arm + leg + hand) は親 → bone レスト方向に沿って置き、ワイヤーフレームが limb 軸に沿うようにし、`BoneShape::CapsuleY` (首 / 背骨 / 胸 / 上胸 / 尻 / 足) は trunk bone のローカル +Y に置き、足は bone の rest world rotation に追従する。半径は実メッシュから計算 — `VrmPrimitive` が `Vec<MeshVertex>` を保持するようになり (load 時に clone)、`collider::collect_weighted_world_positions_into` が「支配的スキン重みが対象 bone に対する値で `VERTEX_WEIGHT_THRESHOLD = 0.25` 未満」の頂点を除外する。スキニング対象外 bone と `MIN_BONE_RADIUS = 0.025` 未満の小さい bone (指) はスキップ。`PhysicsWorld::add_character_bone_colliders` で world 原点の単一 kinematic body にぶら下げ。`update_character_bone_positions` が `current_bone_poses` (これが `model.nodes.world_positions` を読み、`update_skin_palette` が毎フレーム更新し、*加えて* per-bone rest → current world rotation を読む) → 各 collider の `set_translation_wrt_parent` *と* `set_rotation_wrt_parent` でローカル更新。アニメーション追従 (位置と姿勢両方)、per-frame GPU readback ゼロ。
   - `apps/ene-desktop-v2/src/platform/` モジュールは PR5.1 で削除済み (Wayland 実装時に復活)。
2. **Wayland input region** — `apps/ene-desktop/src/character_drag/linux/region.rs` を `apps/ene-desktop-v2/src/platform/wayland_region.rs` に移植。
3. **Wayland オフスクリーン マスク キャプチャ** — `apps/ene-desktop/src/character_drag/linux/capture.rs` (581 行、`R8Unorm` + `Readback::texture` + タイルグリッド矩形分解) を `apps/ene-desktop-v2/src/platform/wayland_mask_capture.rs` に移植。第 2 `wgpu::RenderTarget::Image` 上の `MaskCaptureCamera`。
4. **X11 フォールバック** — `CursorOptions::hit_test` + `_NET_WM_STATE_SKIP_TASKBAR` / `_SKIP_PAGER` FFI を `apps/ene-desktop-v2/src/platform/x11_taskbar.rs` に移植。
5. **Linux デバッグ オーバーレイ** — `draw_visible_rect_gizmos` を `apps/ene-desktop-v2/src/platform/wayland_mask_gizmo.rs` に移植。
6. **マスク ダウンサンプル UI 行** — Linux 限定 `cfg` (PR2 ストレージ配線済み、PR5 で消費)。
7. **フレーム ペーサー (PR5.7 — 2026-06-20 出荷済み)** — `Runtime::about_to_wait` は `ControlFlow::WaitUntil(Instant + frame_interval)` (上限付き `target_fps`) または `ControlFlow::Poll` (`0` = Unlimited) を設定。以前は winit 0.30.13 のマルチウィンドウ + `WaitUntil` バグでブロックされていたが、char ウィンドウの render を `about_to_wait` から直接駆動する (`render_char_frame`) ことで迂回し、レンダーが `RedrawRequested` 配送に依存しなくなった。`apps/ene-desktop/src/scene.rs::pace_frame_rate` のレガシー実装を置き換え — 詳細は §0 PR5.7。

### PR5.5 — レガシー削除

- `apps/ene-desktop` を `apps/ene-desktop-v1` にリネーム → 1 リリース置いて削除。
- ワークスペース `Cargo.toml` から `bevy_*` 全削除。
- `[patch.crates-io] bevy_winit` および `patches/bevy_winit/` 削除。

### PR6+ — 先送り作業

- **PR6** — VRMA 再生 (`bevy_vrm1::vrma` のクローン)。
- **PR7** — Spring Bone (髪 / 布) — `bevy_vrm1::vrm::spring_bone` の完全移植。
- **PR8** — シャドウ品質切替 (FXAA / SMAA / TAA) と設定 UI ページ追加。
- **PR9** — ドラッグ中の操作性改善 (マルチモニタ処理のスムース化)。

---

## 5. 新規 / 削除ファイル (一覧)

> **執筆時点の状態:** **太字** の項目のみが現在ディスク上に存在する。それ以外はすべて計画であり、対応する PR で降りてくる。PR0 固有の現実 (当初スケッチした 7 モジュール分割の代わりに `apps/ene-desktop-v2/` が 3 ファイルにまとまった点) は §22.3 に記載。

### 5.1 新規 (トップレベル)

- **`crates/ene-vrm/Cargo.toml`** (PR1 ステップ 2、出荷済み)
- **`crates/ene-vrm/src/lib.rs`** (PR1 ステップ 2、出荷済み — スタブのみ)
- `crates/ene-vrm/src/loader.rs` (PR3)
- `crates/ene-vrm/src/model.rs`
- `crates/ene-vrm/src/skeleton.rs`
- `crates/ene-vrm/src/mtoon.rs`
- `crates/ene-vrm/src/expression.rs`
- `crates/ene-vrm/src/look_at.rs`
- `crates/ene-vrm/src/camera.rs`
- `crates/ene-vrm/src/renderer.rs`
- `crates/ene-vrm/src/spring_bone.rs` (PR5 で骨格)
- `crates/ene-vrm/src/vrma.rs` (PR5 で骨格)
- `crates/ene-vrm/src/shaders/mtoon.wgsl`
- `crates/ene-vrm/src/shaders/outline.wgsl`
- `crates/ene-vrm/src/shaders/sky.wgsl`
- `crates/ene-vrm/tests/loader.rs`
- `crates/ene-vrm/tests/skeleton.rs`

### 5.2 新規 (`apps/ene-desktop` 内)

- `src/main.rs` (書き換え)
- `src/app_config.rs` (Bevy を除去し書き換え)
- `src/resources.rs` (挙動変更なし)
- `src/ai_bridge.rs` (Bevy を除去し書き換え)
- `src/tray.rs` (挙動変更なし、シグネチャ整理)
- `src/drag.rs` (`character_drag/mod.rs` から移動)
- `src/runtime/mod.rs`
- `src/runtime/window_slot.rs`
- `src/runtime/surface.rs`
- `src/runtime/input.rs`
- `src/runtime/loop.rs`
- `src/gpu/mod.rs`
- `src/gpu/depth.rs`
- `src/gpu/camera.rs`
- `src/platform/mod.rs`
- `src/platform/windows_hwnd.rs`
- `src/platform/drag_subclass.rs`
- `src/platform/wayland_layer.rs`
- `src/ui/mod.rs`
- `src/ui/page_ai.rs`
- `src/ui/page_character.rs`
- `src/ui/page_graphics.rs`
- `src/ui/widgets.rs`
- `src/character/mod.rs`
- `src/character/cursor.rs`
- `src/character/drag.rs`

### 5.2b 新規 (`apps/ene-desktop-v2` 内、PR0 にて出荷済み)

これが現在ディスク上にある **実際の** ファイル配置である。下記の分割は PR1〜PR5 の進行に伴い降りてくるもので、移行完了後に v2 クレートを `apps/ene-desktop` に移設し §5.2 の配置に置き換わる。

- **`apps/ene-desktop-v2/Cargo.toml`** — 依存をスリム化: `winit`, `wgpu`, `pollster`, `bytemuck`, `glam`, `tracing`, `tracing-subscriber`。`raw-window-handle`, `windows-sys`, `ene-core`, `tokio`, `egui`, `tray-icon` はまだ含まない。
- **`apps/ene-desktop-v2/src/main.rs`** — `tracing_subscriber::fmt` のインストールと `EventLoop::run_app`。
- **`apps/ene-desktop-v2/src/gpu.rs`** — `GpuContext`, `pick_format_and_alpha`, `backend_options` (Windows は DX12 / `DxgiFromVisual`、それ以外は `Backends::PRIMARY`)。
- **`apps/ene-desktop-v2/src/runtime.rs`** — `Runtime`, `WindowSlot`, `RectRenderer`, `ApplicationHandler` impl, `AcquireError`。入力ハンドリングは match arm にインライン化、独立した `input.rs` / `surface.rs` / `rect.rs` モジュールは存在しない。

### 5.3 削除

- `patches/bevy_winit/` (ディレクトリ丸ごと)
- `apps/ene-desktop/src/scene.rs`
- `apps/ene-desktop/src/character.rs`
- `apps/ene-desktop/src/settings_ui/` (`src/ui/` で置換)
- `apps/ene-desktop/src/character_drag/` (ロジックは `src/platform/drag_subclass.rs` へ)
- `apps/ene-desktop/src/platform.rs` (`src/platform/` に分割)
- `apps/tw-test/` (Bevy 透過テストベッド — 現時点では §22.3 の相互参照を成立させるため残置、PR1 で他 Bevy スタックと一緒に削除)

---

## 6. 依存関係の変更

> **執筆時点の状態:** 下の **"完了?"** 列が現状のワークスペースを反映する。`追加` は半分完了 (全ワークスペース依存を宣言済み、実際に利用しているのは `apps/ene-desktop-v2` のみ)。`削除` は未着手 — `bevy` は依然として `apps/ene-desktop/Cargo.toml` にあり、`[patch.crates-io] bevy_winit` もワークスペースに配線されている。

### 6.1 追加 (ワークスペース)

| クレート | バージョン | 用途 | 完了? |
|---------|-----------|------|-------|
| `wgpu` | 29 | wgpu コア (Bevy 0.18 の 27 系から更新) | 完了 (ワークスペース依存; `apps/ene-desktop-v2` が利用) |
| `winit` | 0.30 | イベントループとウィンドウ | 完了 (ワークスペース依存; `apps/ene-desktop-v2` が利用) |
| `egui` | 0.34 | イミディエイトモード UI | 完了 (`apps/ene-desktop-v2` が PR2 から利用) |
| `egui-wgpu` | 0.34 | egui → wgpu レンダラ | 完了 (PR2 から利用) |
| `egui-winit` | 0.34 | egui 入力統合 | 完了 (PR2 から利用) |
| `glam` | 0.33 (`bytemuck` フィーチャ付き) | 線形代数 | 完了 (`apps/ene-desktop-v2` / `ene-vrm` が利用) |
| `gltf` | 1.4 | VRM / glTF パーサ | 完了 (PR3 から `ene-vrm` が利用) |
| `encase` | 0.12 | シェーダ互換の構造体パッキング (UBO) | 完了 (PR3 から利用) |
| `bytemuck` | 1 (`derive` フィーチャ付き) | 安全な `Pod` / `Zeroable` キャスト | 完了 (PR3 / PR4 から利用) |
| `pollster` | 0.4 | 起動用の最小 `block_on` | 完了 (`apps/ene-desktop-v2` が利用) |
| `raw-window-handle` | 0.6 | wgpu サーフェス作成 | 完了 (PR3 から利用) |

### 6.2 維持

- `ene-core`, `ene-memory`, `ene-config`, `ene-provider`, `ene-embedding`, `ene-session`, `ene-tool-*`, `ene-tool-host`, `ene-tool-proto`, `ene-tool-derive`, `ene-common` 関連。
- `tray-icon` (`tray.rs` で直接利用)。
- `tokio`, `serde`, `serde_json`, `figment`, `anyhow`, `thiserror`, `tracing`, `directories`。

### 6.3 削除 (ワークスペース)

> **未着手。** 以下はすべて現存している:

- `bevy`, `bevy_ecs`, `bevy_pbr`, `bevy_winit`, `bevy_egui`, `bevy_vrm1`, `bevy_animation`, `bevy_asset`, `bevy_render`, `bevy_math`, `bevy_mesh`, `bevy_window`, `bevy_input`, `bevy_image`, `bevy_transform`, `bevy_utils`。
- `[patch.crates-io] bevy_winit` および `patches/bevy_winit/` ディレクトリ。

---

## 7. ウィンドウ、サーフェス、イベントループ

### 7.1 イベントループ

- `winit::event_loop::EventLoop::new()` をメインスレッドで `EventLoop::run`。
- `control_flow: ControlFlow::Wait` でアイドル時の CPU を抑える。`WindowEvent::RedrawRequested | WindowEvent::Resumed` で 1 ティックだけ `Poll` に切替。
- `WindowId` キーは winit 由来。`Arc<winit::Window>` と対応する `wgpu::Surface<'static>` を `WindowSlot` にまとめて保持。
- wgpu デバイスは `Resumed` 内では作らず、起動時に `pollster::block_on` で 1 回だけ作る。`Resized` では wgpu の再構成パスに任せる。

### 7.2 サーフェス構成

```rust
fn configure(surface: &wgpu::Surface, device: &wgpu::Device, format: wgpu::TextureFormat, width: u32, height: u32) -> wgpu::SurfaceConfiguration {
    wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: width.max(1),
        height: height.max(1),
        present_mode: wgpu::PresentMode::AutoVsync,
        alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied, // Windows; Linux は Auto
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    }
}
```

プラットフォーム別:

- **Windows (DX12)** — `format = device.adapter.get_supported_surface_formats(...).first()`。`WGPU_DX12_PRESENTATION_SYSTEM=DxgiFromVisual` 下では `CompositeAlphaMode::PreMultiplied` がサポートされ、真のピクセル単位アルファが得られる (B1 解消: 我々がサーフェス / スワップチェイン経路を所有するため)。
- **Linux + X11 (Vulkan)** — wgpu は X11 ビジュアルが ARGB の場合のみ `CompositeAlphaMode::PreMultiplied` を報告する。winit ウィンドウ作成時に 32bit RGBA ビジュアルを強制する。利用不可なら `Auto` にフォールバックし制限を文書化。
- **Linux + Wayland** — §10 参照。

### 7.3 フレーム毎エンコーダ

```rust
let frame = slot.surface.get_current_texture()?;
let view = frame.texture.create_view(&Default::default());
let mut encoder = device.create_command_encoder(&Default::default());
{
    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("frame"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color { r: 0., g: 0., b: 0., a: 0. }),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None, // PR3
        timestamp_writes: None,
    });
    // 3D 描画 (PR3+)
    drop(rp);
}
{
    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { /* 同じ view, load Op::Load, clear なし */ });
    // egui 描画
    drop(rp);
}
queue.submit([encoder.finish()]);
frame.present();
```

2 つ目のパスは `LoadOp::Load` を使い、3D パスの結果の上に egui をブレンドする。

---

## 8. 透過戦略 (プラットフォーム別)

### 8.1 Windows

- `WGPU_DX12_PRESENTATION_SYSTEM=DxgiFromVisual` (`main.rs` で `Instance::new` 前に設定)。
- `WindowAttributes::default().with_transparent(true)`。
- `CompositeAlphaMode::PreMultiplied`。
- ドラッグスルー: 既存の `character_drag/windows.rs` の `SetWindowSubclass` 実装を維持し `src/platform/drag_subclass.rs` に移動。修飾子 (既定: 右マウスボタン) を押下中は `WM_NCHITTEST` で `HTTRANSPARENT` を返す。

### 8.2 Linux + X11 (Vulkan)

- `wgpu::Backends::VULKAN` を強制。
- winit ウィンドウ作成時に 32bit RGBA ビジュアルを強制。`x11rb` クレートでスクリーンの ARGB ビジュアルリストを読み 1 つ選ぶ。winit の `WindowBuilder` が ARGB を選べない場合、`x11rb` 経由で X11 ウィンドウを手動生成し raw ハンドルを `WindowAttributes::with_window` で winit に渡す。
- `_NET_WM_WINDOW_OPACITY` を `x11rb` 経由で `0xFFFFFFFF` に設定し、コンポジタがアルファを 0 に潰すのを防ぐ。
- ドラッグスルー: `shape` 拡張または `_NET_WM_WINDOW_OPACITY` ピクセル単位は 1bit マスクが必要。X11 では既定でドラッグスルー無し、後続 PR で clip-shape を出す。

### 8.3 Linux + Wayland

- `wgpu::Backends::VULKAN` を強制。
- `smithay-client-toolkit` (`sctk`) でコンポジタが対応していれば `zwlr-layer-shell-v1` サーフェスを取得。レイヤは `Layer::Overlay` としフルスクリーンウィンドウ上にも浮かぶようにする。
- `sctk::layer::LayerSurface::with_alpha(0.0)` でピクセル単位アルファを依頼。`sctk::shell::wlr_layer::Anchor::empty()` + `KeyboardInteractivity::None` で既定クリックスルー。
- ドラッグは layer サーフェスの `pointer_motion` / `pointer_button` をアプリで処理し、`layer_surface::Surface::commit()` で位置更新。
- フォールバック: `zwlr-layer-shell-v1` 非提供時は通常の `xdg-shell` にフォールバック。ドラッグスルーはグローバルホットキー "freeze character window" トグルに限定。

### 8.4 macOS

- `wgpu::Backends::VULKAN` を強制。
- `WindowAttributes::with_transparent(true)` と `CompositeAlphaMode::PreMultiplied`。テスト / 文書化対象外。

---

## 9. egui 統合

### 9.1 ウィンドウ毎に 1 コンテキスト

- キャラクターウィンドウは独自の `egui::Context` を持ち、当面未使用 (将来デバッグオーバーレイ用に保持)。
- 設定ウィンドウは独自の `egui::Context` を持ち、全設定ページがそこへ描画。
- 設定ページの "Character" デバッグトグル (計画中) は、キャラクターウィンドウ側のコンテキストを使い egui オーバーレイを表示。

### 9.2 フレームポンプ

```rust
fn pump_egui(slot: &mut WindowSlot, state: &mut AppState) -> egui::FullOutput {
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(
            Pos2::ZERO,
            vec2(slot.config.width as f32, slot.config.height as f32),
        )),
        pixels_per_point: Some(slot.window.scale_factor() as f32),
        time: Some(state.now),
        ..slot.egui_state.take_egui_input(&slot.window)
    };
    slot.egui_ctx.run(raw, |ctx| ui::paint(ctx, &mut state.settings, &mut state.ai))
}
```

### 9.3 同一サーフェスへ描画

§7.3 の 3D パスの後、新規レンダーパスを `LoadOp::Load` で開始し、`egui::Context::tessellate` の `PaintJobs` を `egui_wgpu::Renderer::render` に渡す。

### 9.4 設定ウィンドウのホットキー

- グローバルホットキー (例: `Ctrl+,`) を `winit::EventLoop::run` 内またはプラットフォーム固有 API (Windows: `RegisterHotKey`、Linux: `GlobalShortcutsPortal` zbus) で登録。ホットキーは `AppState` の `bool` をトグルし、次フレームでランタイムが設定 `WindowSlot` を作成 / 破棄。

---

## 10. `ene-vrm` クレート内部

### 10.1 公開 API (骨子)

```rust
pub struct VrmHandle(/* Arc<VrmModelInner> */);

pub struct VrmRenderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pipelines: MtoonPipelines,
    skin_buf: wgpu::Buffer,
    camera_buf: wgpu::Buffer,
    bind_layouts: BindLayouts,
}

impl VrmRenderer {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>, format: wgpu::TextureFormat, depth_format: wgpu::TextureFormat) -> Self;
    pub fn load(&self, path: impl AsRef<Path>) -> Result<VrmHandle, VrmError>;
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        model: &VrmHandle,
        camera: &CameraUniform,
        expressions: &ExpressionState,
        look_at: Option<LookAtTarget>,
    );
}

#[derive(Copy, Clone, encase::ShaderType)]
pub struct CameraUniform {
    pub view: glam::Mat4,
    pub proj: glam::Mat4,
    pub view_proj: glam::Mat4,
    pub eye: glam::Vec3,
}

#[derive(Default)]
pub struct ExpressionState(pub BTreeMap<String, f32>);

pub struct LookAtTarget {
    pub world_position: glam::Vec3,
}
```

### 10.2 ローダ (gltf)

`gltf::Importer::import_path` は同期。初回モデル読み込みは AI ランタイムの `spawn_blocking` でラップする。バンドルされた既定 VRM は、winit イベントループ開始前 (ウィンドウサイズをモデルの AABB に合わせるため) にロードする。

glTF の読み取り:

- `document.skins()` を辿り `Skeleton::from_skin(&skin)`。
- `document.nodes()` を再帰的に辿り、`transform: Transform`, `children: Vec<NodeIndex>`, `mesh: Option<usize>`, `skin: Option<usize>` を持つフラットな `Vec<Node>` を構築。ヒューマノイド (`VRMC_vrm-1.0` humanoid bone `hips`) のルートノードを root とする。
- `document.meshes()` を辿り `Primitive { vertex_buffer, index_buffer, material_index }` をアップロード。頂点レイアウトは MToon 必須: position, normal, tangent (任意), uv0, color, **joints (uvec4)**, **weights (vec4)**。ジョイントは 4 つにパディング、余りは weight 0 に振り分ける。
- `document.materials()` と `KHR_materials_unlit` 拡張を辿り、`pbrMetallicRoughness.baseColorTexture` 等を `MToonUniform` にマップ。

`gltf::image::Data` から RGBA / BCn の生バイトを取得し、`wgpu::util::DeviceExt::create_texture_with_data` でステージングコピーせずアップロード。

### 10.3 MToon シェーダ

`shaders/mtoon.wgsl`:

- **頂点** — `joints`, `weights` を読み `skinned_position = Σ(weight[i] * skin[i] * position)`、`skinned_normal = …` を計算。UV, color はパススルー。
- **フラグメント** — VRM MToon ライティングモデルを 0..1 空間で実装:
  - `lit_factor = pow(dot(N, L), shading_shift)`。
  - `lit_factor` と `shade_toony` の smoothstep で `base_color` と `shade_color` を補間。
  - スペキュラ: `parametric.ramp` 参照の軽量 GGX。
  - エミッション: `emissive` + テクスチャ。
  - アウトライン: 別頂点ストリームで法線方向に `outline_width * (1.0 - lit_factor)` だけ変位。パスは `cull_mode: Front`、`depth_write: false`、`blend: (SrcAlpha, OneMinusSrcAlpha)`。
  - 出力は `vec4(premultiplied_color, alpha)`。ウィンドウレベル `CompositeAlphaMode::PreMultiplied` と MToon 出力を一致させるため `premultiplied_alpha` 出力。
- シェーダには `// SPEC: MToon 1.0 §3.4.1` 等の注釈を付与し、公式仕様と突合できるようにする。

当面は単一直接光モデル。環境項 (matcap / rim) はフォローアップで追加。

### 10.4 表情 / モーフ

- glTF メッシュプリミティブの `targets` フィールドがブレンドシェイプの頂点デルタを保持。`crates/ene-vrm/src/expression.rs` が `PrimitiveMorphs { primitive_id, targets: Vec<MorphTarget>, name_to_slot, target_count, vertex_count }` を定義し、ローダは `primitive.reader(...).read_morph_targets()` の POSITION 変位をローダの `scale` のみで正規化して詰める(**`(p - center) * scale` ではない** — モーフデルタは線形量なので `-center` で変換すると、重み付き頂点がすべてモデル中心方向にドラッグされる)。`loader::normalize_morph_offset` および回帰テスト `morph_offset_is_not_translated_by_model_centre` を参照。
- モーフ名は **`gltf::mesh::MorphTarget` の公開ビューには存在しない**。ローダは `VRMC_vrm` エクステンション ツリーから取り出す: `Document::extensions()["VRMC_vrm"]["expressions"].{preset,custom}.<name>.morphTargetBinds[*]` (各 bind は `{node, index}`、`Node::mesh().index()` で解決)。名前はそのメッシュの **全** プリミティブにバインドされる (仕様では「全プリミティブが同じ morphTarget を共有する」)。エクステンションが参照しないターゲットは `morph_target_<i>` にフォールバック。
- モーフを持つプリミティブごとに 1 ストレージバッファを確保: `morph_offsets: array<vec3>` (長さ `target_count * vertex_count`)、`morph_offsets[target_index * vertex_count + vertex_index]` で索引する。モーフを持たないプリミティブは単一のダミー レイアウト (`target_count = 0`) を共有し、シェーダの `if (target_count > 0u)` 早期脱出で storage lookup をスキップ。
- 表情名 (例: `happy`, `sad`, `blink`) ごとに、`ExpressionState` / `ExpressionLayer::weights` のキーをモデルが解決した名前で事前投入する。`VrmModel::set_expression(name, weight)` がランタイムの書き込み経路、レンダラは `model.expressions().weights.get(name)` で読み取る。
- 頂点シェーダは `morph_meta: MorphMeta { vertex_count, target_count, _pad0, _pad1, weights: array<vec4<f32>, 16> }` を取る。パック済み `weights` 配列は `vec4` あたり 4 個のモーフ重みを保持 (16 vec4 = 64 スロット/プリミティブ)。`position += Σ_t( weights[t/4][t%4] * morph_offsets[t * vertex_count + vidx] )` を適用する。
- **後回し (PR4.5+)**: 法線 / 接線 モーフ変位、マルチターゲット ブレンドシェイプ グラフ (例: `blink_l + blink_r → blink`)、LookAt `expression` モード (`lookLeft/Right/Up/Down` を重みマップに直接書き込み)。

### 10.5 LookAt

- VRM 1.0 の meta に `lookAt` オブジェクト。読み取る:
  - `lookAtType` (`bone` or `expression`)。
  - `rangeMapHorizontalInner` / `rangeMapHorizontalOuter` / 垂直の同類 (ヨー / ピッチのクランプに使用)。
- `bone` モードでは humanoid ボーン階層 (`head`, `leftEye`, `rightEye`) を辿り小さなクォータニオンデルタを適用。`bevy_vrm1` の `compute_look_at_slerp` 着想を `glam::Quat::slerp` で再利用。
- `expression` モードではターゲット方向から `lookLeft`, `lookRight`, `lookUp`, `lookDown` の重みを `ExpressionState` に与える。ボーンマス計算なし。

### 10.6 Body Tracking (カーソル追従)

- `apps/ene-desktop/src/character/cursor.rs` が OS カーソルのスクリーン座標をカメラ前方の固定深度 (1.5m) の `Vec3` に変換し `LookAtTarget` として渡す。
- `ene-vrm` はワールド位置をモデルローカル空間に変換し `solve_look_at` (PR4) を呼び、新しい頭 / 目変換を返す。PR4 では脊椎や肩は変更しない。

### 10.7 テスト (`crates/ene-vrm/tests/`)

- `loader.rs` — 合成ミニマル glTF を読み込み、スキニング行列、ジョイント数、マテリアルパラメータを検証。
- `skeleton.rs` — 既知ボーンポーズのレストポーズでスキニング行列を検証。
- `expression.rs` — ブレンドシェイプに 1.0 のウェイトを設定し、オフセットバッファ長とサンプルオフセットを検証。
- `look_at.rs` — 頭ボーンを与えターゲットを向かせ、計算後の頭の前方ベクトルが正規化方向の ε 以内であることを検証。

実 GPU を必要としないよう、テストデバイスは `wgpu::util::initialize_adapter_from_env_or_default` を `wgpu::Features::empty()`, `wgpu::Limits::downlevel_defaults()` で取得。AGENTS.md に従い GPU がない CI では `#[ignore]`。

---

## 11. AI ブリッジ書き換え

`apps/ene-desktop/src/ai_bridge.rs`:

```rust
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use ene_core::api::{EneEvent, EneHandle};

#[derive(Clone)]
pub struct AiBridge {
    handle: EneHandle,
    inner: Arc<Inner>,
}

struct Inner {
    receiver: tokio::sync::mpsc::UnboundedReceiver<EneEvent>,
    processing: AtomicBool,
    pending: Mutex<VecDeque<EneEvent>>,
    emotion_queue: Mutex<VecDeque<EmotionSample>>,
}

impl AiBridge {
    pub fn spawn(handle: EneHandle) -> Self { /* タスク起動 */ }
    pub fn is_processing(&self) -> bool { self.inner.processing.load(Ordering::Acquire) }
    pub fn drain(&self) -> Vec<EneEvent> {
        std::mem::take(&mut *self.inner.pending.lock().unwrap())
    }
    pub fn latest_emotion(&self) -> Option<EmotionSample> { /* キューから pop */ }
}
```

デスクトップランタイム上の `tokio::spawn`:

```rust
async fn pump(mut receiver: mpsc::UnboundedReceiver<EneEvent>, inner: Arc<Inner>) {
    while let Some(ev) = receiver.recv().await {
        match &ev {
            EneEvent::RunStart => inner.processing.store(true, Ordering::Release),
            EneEvent::RunEnd { .. } => inner.processing.store(false, Ordering::Release),
            _ => {}
        }
        inner.pending.lock().unwrap().push_back(ev);
    }
}
```

ランタイムは毎フレーム `ai_bridge.drain()` を呼び、`ui::paint` (テキストデルタ、ラン境界) と `character` (感情) に振り分ける。

---

## 12. システムトレイ、ホットキー、設定永続化

- **トレイ** — `tray-icon` 0.x。Windows では既存の専用 `GetMessage` スレッドを維持、Linux では GTK メインループ。トレイメニューは `Arc<AppState>` を捕捉するクロージャで再構築され、"Settings", "Reload VRM", "Quit" は機能維持。
- **ホットキー** — Windows は `RegisterHotKey` (`character_drag/windows.rs` から移動)。Linux は `org.freedesktop.portal.GlobalShortcuts` の zbus インタフェース (Desktop ポータル用に追加済みで未使用だった `zbus` を活用)。
- **設定永続化** — `ene_config::CharacterSettings` を起動時に 1 回読み、設定ウィンドウの egui ページから書き戻す。挙動は不変。

---

## 13. ドラッグ中 / ウィンドウパススルー

### 13.1 Windows

- `platform::drag_subclass` がウィンドウ作成時にサブクラスを登録。`WM_NCHITTEST` を処理し、設定修飾子 (既定: 右マウスボタン) を押下中は `HTTRANSPARENT` を返す。それ以外の入力は winit イベントループへ。
- ドラッグ時のみ低レベルマウスフック (`SetWindowsHookExW(WH_MOUSE_LL, …)`) を仕掛け、修飾子状態をグローバルに参照可能にする。

### 13.2 Linux

- **X11** — `_NET_WM_WINDOW_OPACITY` をグローバル設定。真のヒットテストパススルーは `shape` 拡張で実現。PR1 では no-op スタブを出し、PR8 で有効化。
- **Wayland** — `sctk` サーフェスの `pointer_enter` / `pointer_leave` でグラブ可否を決定。ドラッグはサーフェスのインタラクティブ領域 (layer-shell がサポートする場合) または修飾子認識の領域で実装。1 ピクセルの透明リングは Wayland では不可。

---

## 14. 設定とスキーマ

- `assets/character_settings.schema.json` と `assets/settings.schema.json` は `ene-config` の `define_config!` マクロで自動再生成される。本作業では変更しない。
- 新規トップレベルキーは追加しない (`vrm.file` は既存維持)。
- グラフィックスは `vulkan_forced: bool` (Windows 規定 `false`、Linux 規定 `true`) に簡素化。FXAA / SMAA / TAA トグルは PR1 で削除、`vrm.outline_width` と `vrm.look_at_clamp_*` は PR4 向けに維持。

---

## 15. エラーハンドリング、ロギング、パニック

- wgpu エラーは単一の `wgpu::ErrorScope` に通し `tracing::error!` する。`RequestDeviceError` や `SurfaceError` を `unwrap()` しない。
- `wgpu::SurfaceError::Lost | Outdated | OutOfMemory` は `slot.recreate_surface()` を起動 (同じ `Arc<Window>` から wgpu サーフェスを再生成)。
- トレイスレッド (Windows) と GTK スレッド (Linux) はパニック禁止。`mpsc::Sender<AppEvent>` でランタイムに通知 (`AppEvent::TrayAction(TrayAction)`, `AppEvent::Quit`)。
- AI ブリッジタスクはデスクトップランタイム唯一の長寿命 tokio タスク。パニック時はログ出力の上 3 回まで自動再起動し、超過時はトレイのステータスアイコンを赤に反転して終了誘導。

---

## 16. テスト戦略

### 16.1 ユニットテスト

- `crates/ene-vrm` — loader, skeleton, expression, look_at (§10.7)。
- `apps/ene-desktop/src/runtime/surface.rs` — `mock_device()` ヘルパでサーフェス構成ロジック。
- `apps/ene-desktop/src/ai_bridge.rs` — チャネル + drain セマンティクス (GPU 不要)。

### 16.2 統合テスト

- `crates/ene-vrm/tests/gltf_roundtrip.rs` — `gltf-json` でインメモリ glTF を構築し `tempfile::TempDir` に書き出し、`VrmRenderer::load` の成功を検証。
- `apps/ene-desktop/tests/window_lifecycle.rs` は `#[ignore]`。実ディスプレイ必須、Linux で手動実行。

### 16.3 手動スモークチェックリスト (PR 毎)

PR1:
- [ ] Windows で `DxgiFromVisual` 下アプリ起動、透過ウィンドウ表示。
- [ ] Linux + X11 + Vulkan で起動、ウィンドウ表示。
- [ ] Linux + Wayland (GNOME / KDE) + Vulkan で起動、ウィンドウ表示。
- [ ] トレイアイコン表示、"Quit" 動作。
- [ ] `cargo clippy --workspace -- -D warnings` クリーン。

PR2:
- [ ] 設定ウィンドウがトレイメニューで開き、描画され、綺麗に閉じる。
- [ ] 設定ウィンドウを閉じてもキャラクターウィンドウに影響なし。
- [ ] 設定ウィンドウのリサイズで再構成。

PR3:
- [ ] 既定 VRM が描画される。カメラ距離が妥当。ウィンドウリサイズで再描画。
- [ ] GPU 検証警告なし。
- [ ] `cargo test -p ene-vrm` パス。

PR4:
- [ ] カーソル移動が頭 / 目方向に反映。
- [ ] AI 感情テキスト → ブレンドシェイプ遷移がスムーズ (1 フレームのスナップなし)。
- [ ] 複数の感情入力が連続して合成される。

### 16.4 CI

- `cargo fmt --all --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace` (GPU テストは `#[ignore]`)
- `cargo build --workspace --release`

CI ではディスプレイサーバ依存のステップは当面実行しない。ランナーが用意できた段階で Wayland/X11 スモークを別ワークフロー化。

---

## 17. リスクと対策

| リスク | 可能性 | 影響 | 対策 |
|--------|--------|------|------|
| 特定 Windows GPU ドライバで `DxgiFromVisual` 非対応 | 低 | 大 (PR1 検証不能) | 起動時検出。非対応時は `Auto` フォールバックして警告ログ。ドライバ更新または wgpu アップグレードで解決。 |
| Wayland コンポジタが `zwlr-layer-shell-v1` を公開しない (古い Sway 等) | 中 | 中 | `xdg-shell` フォールバックし制限を文書化。 |
| MToon シェーダ出力が bevy_vrm1 と視覚的に乖離 | 高 | 中 | PR3 マージ前に既存アプリと並列描画テスト。同既定 VRM を使用。 |
| wgpu 0.x リリースでマイナー互換が壊れる | 中 | 小 | 27.x が安定するまで `wgpu = "27.0.1"` で完全ピン留め。 |
| `egui-wgpu` 0.33 が `bevy_egui` 0.39 と API 差分 | 中 | 小 | egui-wgpu リポジトリの example を真実とする。小さな `EgUiWgpuHelper` を一度書く。 |
| `bevy_winit` パッチ削除が無関係ツールを壊す | 低 | 小 | パッチは `bevy_winit` 専用。他ツールは依存していない。 |

---

## 18. オープンな問い / 今後の作業

- **Q1.** シェーダ合成に `naga-oil` を使うか、生 WGSL 維持か。— 既定: 生 WGSL。必要なら将来 `naga-oil` の include 対応を追加。
- **Q2.** 設定ウィンドウとキャラクターウィンドウの egui コンテキストを共有するか。— 共有しない (明確化と将来的なウィンドウ毎スタイリングのため分離維持)。
- **Q3.** `ene-vrm` を wasm-bindgen ターゲットで公開するか。— 先送り。MToon シェーダは可搬だが wasm での wgpu デバイス選定は非自明。
- **Q4.** Spring Bone のステップ数 — bevy_vrm1 の既定は 8 反復 / 60Hz。PR6 で `vrm.spring_bone.*` として公開。
- **Q5.** VRMA — PR5 後、独自 VRMA パーサを維持するか公式 `vrma` クレートを使うか決定。公式は無いため当面独自実装。

---

## 19. 用語集

- **VRM** — glTF ベースのアバタフォーマット。v1.0 は `VRMC_vrm-1.0` 拡張。v0.x は `VRMC_vrm-0.x` を使用し **本移行のスコープ外**。
- **MToon** — VRM 定義のセルシェーディングマテリアル。入力: base color, shade color, shading shift, rim, matcap, outline。
- **Pre-multiplied alpha** — 出力が `(R*A, G*A, B*A, A)`。wgpu の `CompositeAlphaMode::PreMultiplied` スワップチェインでピクセル単位アルファを尊重するために必要。
- **DxgiFromVisual** — wgpu の DX12 バックエンドオプション。HWND のビジュアルからスワップチェインを生成し、`WS_EX_LAYERED` のピクセル単位アルファを得るために必須。
- **layer-shell** — Wayland プロトコル (Waybar 等が利用)。ピクセル単位アルファと `Above` レイヤリングを提供。
- **Spring bone** — VRM 0.x / 1.x の髪 / 布などの二次運動。単純なバネ - ダンパシミュで駆動。
- **VRMA** — VRM Animation。glTF ベースのアニメーションクリップフォーマット。

---

## 20. 付録: Bevy の概念と新スタックの対応

| Bevy 0.18 | 新コード |
|-----------|---------|
| `App`, `DefaultPlugins` | `runtime::AppState` + 手書き `winit::EventLoop` |
| `Window`, `WindowPlugin` | `winit::window::Window` + `runtime::window_slot::WindowSlot` |
| `RenderDevice`, `RenderQueue` | `wgpu::Device`, `wgpu::Queue` (`gpu::Context`) |
| `Camera3d` | `gpu::camera::Camera` (当面は正射影) |
| `Mesh`, `MeshPlugin` | `ene_vrm::model::MeshGpu` |
| `StandardMaterial` | `ene_vrm::mtoon::MToonMaterial` |
| `EguiPlugin` | `ui::paint` + `runtime::window_slot::WindowSlot` の egui フィールド群 |
| `EguiMultipassSchedule` | `runtime::loop` 内のウィンドウ単位「3D 描画 → egui」ループ |
| `VrmPlugin` | `ene_vrm::VrmRenderer` (1 度だけ構築) |
| `VrmaPlugin` | PR5+ |
| `Messages<T>` | `tokio::sync::mpsc` + `Arc<Mutex<VecDeque>>` |
| `Resource<T>` | `AppState` フィールド |
| `Local<T>` | `WindowSlot` 単位のフィールド |
| `Query<>` | `WindowSlot` の直接フィールドアクセス |
| `Time<()>` | `std::time::Instant` (`AppState`) |
| `WinitSettings::desktop_app()` | `EventLoop::set_control_flow(ControlFlow::Wait)` + `set_redraw_requested` |
| `bevy::tasks::TaskPool` | `tokio::runtime` (既設) |
| `bevy::log` | `tracing` (既設) |

---

## 21. サインオフチェックリスト (PR 毎)

- [ ] `cargo fmt --all`
- [ ] `cargo clippy --workspace -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo build --workspace --release`
- [ ] 当該 PR スコープの手動スモーク (Windows + Linux + Wayland)
- [ ] 英語ドキュメントを `docs/` 配下更新
- [ ] 日本語ドキュメントを `docs/ja/` 配下更新
- [ ] `docs/architecture/wgpu-migration.md` を新状態に合わせて更新 (本ファイル)

---

## 22. PR1 実装メモ

### 22.1 実機 Windows で判明した落とし穴: `Opaque` のみ対応サーフェス — **§22.3 で訂正済み**

> **§22.3 (PR0) で訂正済み。** 本節にある "正しい診断" も全部古く、 本当の根本原因は「wgpu 27 は `WGPU_DX12_PRESENTATION_SYSTEM` env var を自動では読まない」 — アプリ側が `Dx12SwapchainKind` を直接 `BackendOptions::dx12::presentation_system` に設定し、 `Instance::new` に渡す必要がある (Bevy 0.18 は `bevy_render/src/renderer/mod.rs:201` で、 v2 は `apps/ene-desktop-v2/src/gpu.rs::backend_options` でそうしている)。 詳細は §22.3 を参照。

旧 PR1 試行 (DX12 env var + `WS_EX_LAYERED` nudge、 Vulkan 入れ替え) と中間の「`WS_EX_NOREDIRECTIONBITMAP` 不足」診断は git 履歴には残るが、 本ファイルを読む人には不要。 外部からの相互参照が 404 にならないよう 1 行ポインタとして残す。

### 22.2 PR1 ファイル単位の状態 (執筆時点)

| アクション | ファイル / ディレクトリ | 状態 |
|------------|------------------------|------|
| **新規** | `crates/ene-vrm/Cargo.toml` | 出荷済み (PR1 ステップ 2) |
| **新規** | `crates/ene-vrm/src/lib.rs` | 出荷済み (PR1 ステップ 2 — `pub fn version()` スタブ + ユニットテスト 1 件) |
| **新規** | `apps/ene-desktop-v2/Cargo.toml` | 出荷済み (PR0) |
| **新規** | `apps/ene-desktop-v2/src/{main,gpu,runtime}.rs` | 出荷済み (PR0 — v2 スモーク完成) |
| **書き換え** | `apps/ene-desktop/src/{main,app_config,ai_bridge,tray}.rs` | **未着手** — 既存 Bevy バイナリは無修正 |
| **書き換え** | `apps/ene-desktop/Cargo.toml` | **未着手** — 依然 Bevy 0.18 |
| **削除** | `apps/ene-desktop/src/{scene,character,platform}.rs` | **未着手** |
| **削除** | `apps/ene-desktop/src/{settings_ui,character_drag}/` | **未着手** |
| **削除** | `patches/bevy_winit/` | **未着手** — パッチと `[patch.crates-io]` が `Cargo.toml` に残置 |
| **削除** | `apps/tw-test/` | **未着手** — Bevy テストベッドが透過レシピの相互参照として残置 |
| **ワークスペース** | `Cargo.toml` のレンダリングスタック節 | **部分** — 依存は追加済み (PR0)、`[patch.crates-io] bevy_winit` は未削除 |

上層の意図 ("PR1 が削除ステップ") は変わらない。PR0 が進めた行だけを出荷済みとマークしている。

### 22.3 PR0 — Minimum v2 transparency smoke

> **ステータス:** 出荷済み。 この開発機で §8.1 の Windows 透過レシピがエンドツーエンドで動くことを示す最小 `apps/ene-desktop-v2` スケフォールド。 (誤りだった) §22.1 診断を置き換える。

**v2 の配置 (3 ファイル / 約 330 行):**

```text
apps/ene-desktop-v2/
├── Cargo.toml        # winit, wgpu, pollster, bytemuck, glam, tracing, tracing-subscriber
└── src/
    ├── main.rs       # tracing-subscriber 初期化 + EventLoop::run_app
    ├── gpu.rs        # GpuContext, pick_format_and_alpha, backend_options (DX12 / DxgiFromVisual)
    └── runtime.rs    # Runtime, WindowSlot, RectRenderer, ApplicationHandler, AcquireError
```

当初 §5.2 で計画した 7 モジュール分割 (`runtime/{mod,input,surface,window_slot,loop,rect}.rs`, `gpu/{mod,depth,surface_format}.rs`, `platform/{mod,...}.rs`) は上記の 3 ファイルに集約した。理由は §22.3 末尾の "PR0 のファイル" ブロック参照。

**目的:** winit + wgpu 27 の Windows ウィンドウで、 (a) 透過 (DWM が swapchain の per-pixel α を尊重する)、 (b) 単色の矩形を 1 つ描画、 (c) `Space` で透過 / 不透明を切替、 `Escape` で終了。 egui / VRM / AI ブリッジなし。 純粋な描画スモーク。

**ようやく動いたレシピ (4 つ全部揃えること):**

1. **`wgpu::Dx12SwapchainKind::DxgiFromVisual`** — `apps/ene-desktop-v2/src/gpu.rs::backend_options` で `BackendOptions::dx12::presentation_system` に直接設定し、 `wgpu::Instance::new` に渡す。 wgpu 27 DX12 バックエンドが HWND の visual から swapchain を作るためのオプションで、 per-pixel α に必要。 **`WGPU_DX12_PRESENTATION_SYSTEM` 環境変数は単独では効果なし** — wgpu 27 は `Dx12SwapchainKind::from_env()` の中だけで env を読むが、 v2 はそれを呼ばない。 `DxgiFromVisual` が無いと wgpu の DX12 サーフェスは `SurfaceTarget::WndHandle` で作られ、 `Surface::get_capabilities` は `[CompositeAlphaMode::Opaque]` だけを返す (`wgpu-hal-27.0.4/src/dx12/adapter.rs:1006-1018` 参照)。 これが PR0 でずっと出ていた「不透明な黒」の根本原因。

2. `WindowAttributesExtWindows::with_no_redirection_bitmap(true)` — `apps/ene-desktop-v2/src/runtime.rs::window_attributes` の `WindowAttributes` ビルダーで `transparent` の値に関わらず常に設定。 HWND の exstyle に `WS_EX_NOREDIRECTIONBITMAP` (0x00200000) を create 時点で追加する。 **§22.1 の `force_layered_window` アプローチが取りこぼしていたのはこのピース。** `WS_EX_LAYERED` だけ立てても wgpu は `PreMultiplied` を advertise するが、 DWM はリダイレクションバitmap 経由でコンポジットし、 swapchain の per-pixel α を読まないため背景が不透明な黒になる。 両スタイルが揃って初めて成立する:

   ```text
   WS_EX_LAYERED            = 0x00080000   (with_transparent(true) で追加)
   WS_EX_NOREDIRECTIONBITMAP = 0x00200000   (with_no_redirection_bitmap(true) で追加)
   ```

   > **WARNING — PR0 クロージング後の更新。** winit 0.30.x の `with_transparent(true)` は HWND に `WS_EX_LAYERED` を **追加しない** (セットされるのは `WindowFlags::TRANSPARENT` のみで、 これはレガシー DWM blur-behind 経路でだけ参照されるが、 `with_no_redirection_bitmap(true)` も立っているとその経路はスキップされる)。 winit 0.30 が exstyle に `WS_EX_LAYERED` を足すのは `with_ignore_cursor_events(true)` 経路だけ。 **作成後のフックから `SetWindowLongPtrW(WS_EX_LAYERED)` で補おうとしてはならない** — PR0 開発機では、 `WS_EX_NOREDIRECTIONBITMAP` の上にそれを足すと DWM がコンポジションを不透明な黒に戻した (レガシー layered 経路が non-redirected 経路を上書きしてしまうためと思われる)。 `apps/ene-desktop-v2/src/platform/windows.rs` は PR0 で **診断専用** ヘルパ (`log_window_styles`) に降格し、 exstyle をミューテートしない方針に統一。 `with_no_redirection_bitmap` を `window_attributes` で叩く以外の Win32 変更はしない。

3. `CompositeAlphaMode::PreMultiplied` を `SurfaceConfiguration` に — **プラットフォームから直接 picking** ( `SurfaceCapabilities::alpha_modes` を見ない )。 旧実装は caps を走査して `PreMultiplied` / `PostMultiplied` を探し、 どちらも無ければ `Opaque` にフォールバックしていた — これが PR0 でずっと出ていた「不透明な黒」のもう片方の根本原因で、 サーフェスがエラーを返さず黙って `Opaque` に degrade していた。 現実装は Windows / Linux では `CompositeAlphaMode::PreMultiplied`、 macOS では `CompositeAlphaMode::PostMultiplied` を無条件で返す。 `apps/tw-test` と完全に一致。 本当にサポートされていない場合は `Surface::configure` が `PreMultiplied` で呼ばれ、 wgpu 27 が次の `get_current_texture` で失敗する。 `Runtime::window_event` の `AcquireError::Reconfigure` 経路が明確な `WARN` をログして reconfigure するので失敗は黙らない。 `WindowSlot::new` も、 要求した alpha_mode が caps に無いとき即座に `WARN` を出す — サーフェス設定ミスを 1 ログ行で検知できる。

4. 透過モードで `(0, 0, 0, 0)`、 不透明モードで `(0.2, 0.2, 0.2, 1.0)` にクリア — `WindowSlot::render_frame` を参照。 赤矩形は同じパス内で `LoadOp::Load` で重ねて描画される (クリアが透けて見える)。

**動作中の `apps/tw-test` レシピとの対照:** パッチ済み `bevy_winit` が `patches/bevy_winit/src/winit_windows.rs:133-146` で `with_transparent(true)` と `with_no_redirection_bitmap(true)` を一緒に呼んでいる。 PR0 はそのレシピをパッチ無しで v2 に再実装。

**起動時の診断ログ** (`gpu::GpuContext::new` / `WindowSlot::new` から出力。 `tracing-subscriber` を `main.rs` の冒頭で `EnvFilter("info,wgpu_core=warn,wgpu_hal=warn,naga=warn")` デフォルトで初期化 — `RUST_LOG` で上書き可。 これにより次回以降のリグレッションが 1 ログ行で検知できる):

```text
INFO  wgpu surface capabilities: formats=[Bgra8UnormSrgb], alpha_modes=[Opaque, PreMultiplied, PostMultiplied, Inherit, Auto]
INFO  SurfaceConfiguration picked: format=Bgra8UnormSrgb, alpha_mode=PreMultiplied
```

`caps.alpha_modes` が `[Opaque]` のみの場合、 picker が明示的な `WARN` を出し、 そのまま `Surface::configure` を `alpha_mode=PreMultiplied` で呼ぶ。 1 フレーム目で `SurfaceError::Outdated` または `Lost` になり、 input ループが「Surface acquire returned Outdated/Lost」を連続ログ、 ユーザには設定ミスを指す明確な `WARN` チェーンが見える。 それが DX12 / wgpu / ホスト環境を直すか、 PR0 のスコープ外である `UpdateLayeredWindow` の手動 GDI 経路に逃げる合図。 `caps.alpha_modes` に `PreMultiplied` があるのに表示が不透明な黒のままなら、 まず `NO_REDIRECTION_BITMAP=true` の行を確認。 `NO_REDIRECTION_BITMAP=false` なのに `with_no_redirection_bitmap(true)` を呼んでいるなら、 使用中の winit バージョンがフラグを無視している — ワークスペース `Cargo.lock` で winit 0.30 + `WindowAttributesExtWindows` トレイトが入っているか確認。

**デフォルト起動状態:** `Runtime::new` は `transparent = false` で初期化 (灰色の不透明ウィンドウ + 装飾 + 赤矩形)。 ユーザーが `Space` で透過を試せる。 透過が効かない環境でもウィンドウ自体は正しく見える (グレー + 赤矩形) ので、 描画パイプラインが生きていることを目視で確認できる UX セーフネット。

**キーボード / ライフサイクル (PR0 スコープ、 `Runtime::window_event` 参照):**
- `Space` — `transparent` をトグル。 `Window::set_decorations(!transparent)` を呼ぶ。 `WindowSlot::render_frame` のクリア色が切り替わる。
- `Escape` または閉じるボタン — `EventLoop::exit()`。
- `Resized` / `ScaleFactorChanged` — `WindowSlot::reconfigure` 後 `request_redraw`。
- `RedrawRequested` — `WindowSlot::render_frame`。 `SurfaceError::Outdated | Lost` の場合 reconfig 後再 redraw。

**PR0 のファイル (`git diff --stat` 参照):**
- `apps/ene-desktop-v2/Cargo.toml` — `winit`, `wgpu`, `pollster`, `bytemuck`, `glam`, `tracing`, `tracing-subscriber` (env-filter + fmt) にスリム化。 `raw-window-handle`, `windows-sys`, `ene-core`, `tokio`, `egui`, `tray-icon` なし。
- **レイアウト: 3 ファイル / 約 330 行**。 `src/main.rs` (tracing 初期化 + イベントループ)、 `src/gpu.rs` (`GpuContext` + `pick_format_and_alpha` + DX12 backend options)、 `src/runtime.rs` (`Runtime` + `WindowSlot` + `RectRenderer` + `ApplicationHandler` impl + `AcquireError`)。
- 削除: `src/gpu/{mod,surface_format}.rs` (gpu.rs に統合)、 `src/platform/{mod,linux,windows}.rs` (HWND exstyle 診断ログ削除; Linux ディスプレイサーバログ削除 — どちらも nice-to-have でレシピには不要)、 `src/runtime/{mod,input,surface,window_slot,rect}.rs` (runtime.rs に統合)。
- `RectRenderer` 簡素化: UBO / bind group / bind 付き pipeline layout なし。 NDC `[-0.5, 0.5]²` クアッド (6 頂点 `TriangleList`)、 色は WGSL 内でハードコード、 pipeline layout は空。 元の 211 行レンダラが ~80 行に。
- 入力ハンドリングは `ApplicationHandler::window_event` の match arm にインライン化。 7 引数の `input::route` は消滅、 `toggle_transparency` ロジックは `Space` arm 内に。
- `main.rs` — `tracing_subscriber::fmt` を他の処理より先にインストールし、 wgpu caps とサーフェス format が起動時にログされる。

**手動スモーク (本開発機で検証済み):**
1. `cargo run -p ene-desktop-v2` — 灰色の不透明ウィンドウにタイトルバー、 中央に赤矩形。
2. `Space` を押す — ボーダーレス、 背景が実際のデスクトップに、 赤矩形はそのまま。
3. もう一度 `Space` — グレー + タイトルバーに戻る。
4. `Escape` — きれいに終了。
5. リサイズ — フレーム再構築、 赤矩形は NDC 原点に留まる。

**既知の制限 (PR1+ へ持ち越し):**
- v2 は単一ウィンドウスロットのみ。 設定ウィンドウ、 トレイ、 AI ブリッジ、 VRM レンダラは PR1+。
- wgpu 27 のサーフェスフォーマット picker はもう `Opaque` にフォールバック **しない**。 ホストの DXGI が `PreMultiplied` を advertise しない場合、 1 フレーム目の acquire が失敗し、 input ループが `WARN` をログし続けてリグレッションが即座に分かる。
- `force_layered_window` ヘルパは診断 no-op として残置。 実際の透過有効化は `window_attributes` の `with_no_redirection_bitmap` 呼び出し。

---

## 22.4 PR1 — v2 トレイ + AI ブリッジ + `AppState` + 永続化 + CLI

> **ステータス:** **出荷済み。** v2 は tokio ランタイムを起動し、tokio 駆動の `EneHandle` アクター、システムトレイ (`tray-icon` 0.24)、および `CharacterSettings` スキーマ (レガシーの Bevy `Resource` からプレーンな `Arc<parking_lot::RwLock<…>>` 構造体に移植) を保持するようになりました。レガシーの `apps/ene-desktop` (Bevy 0.18) は、新しい §0.1 / §4 ポリシーに従い、意図的に変更していません。概要については §0 の表を、ステップバイステップの計画については §4 PR1 を参照してください。

### なぜ「v2 が完全な同等機能に成長してから名前を変更する」のか (古い「PR1 は削除ステップ」の代わり)

当初の計画では「PR1 が削除ステップ」であり、`apps/ene-desktop` から Bevy を取り除き、そこに v2 のソースを移動することになっていました。しかし、このポリシーのリスクは、どのコミット時点においても、一方のバイナリで機能が動き、もう一方では動かない、あるいはどちらのバイナリも起動しないという状態が発生し得ることです。新しいポリシー (§0.1 参照) では、**移行全体を通じて両方のバイナリが並行してビルド可能であること**とします。v2 は PR1 から PR5+ にかけて段階的に成長し、レガシーソースの名前変更と削除は PR5 の最後 (PR5.5) に単一のコミットで行われます。この2つのクレートの共存は、移行期の一時的な状態ではなく、プロジェクトのデフォルトモードになります。

これは開発者体験のみの変更です。ユーザーの視点からは、PR5.5 で Bevy バイナリが消え、v2 バイナリが起動するようになるまで、移行は目に見えません。

### モジュールマップ (執筆時点で 8 ファイル、約 1,500 行)

```text
apps/ene-desktop-v2/src/
├── main.rs       # tracing 初期化 + tokio ランタイム + AppState::with_channel + EventLoop::run_app
├── gpu.rs        # GpuContext, pick_format_and_alpha, backend_options (DX12 / DxgiFromVisual)
├── runtime.rs    # Runtime, CharacterWindow, UiWindow, RectRenderer, ApplicationHandler
├── state.rs      # AppState (gpu, settings, ai, tray, event_rx), AppStateError, with_channel
├── events.rs     # AppEvent, AiStreamUpdate, AppEventSender/Receiver (tokio mpsc)
├── settings.rs   # CharacterSettings (プレーンな構造体、レガシーの app_config.rs から移植)
├── ai_bridge.rs  # EneHandle をラップする AiBridge, tokio タスク: pump_events, bootstrap
└── tray.rs       # TrayHandle (Windows: GetMessageW スレッド; Linux: GTK ポンプ + 受信スレッド)
```

### 主要な設計決定

1. **`CharacterSettings` はプレーンな構造体であり、`Resource` ではありません。** レガシーの Bevy アプリは、自動的な `Arc` ラッピングと `Deref` のエルゴノミクスを提供する `bevy::ecs::resource::Resource` を使用していました。v2 には Bevy がないため、代わりに `Arc<parking_lot::RwLock<CharacterSettings>>` を使用します。この `Arc` は、アクターの再構成を駆動するために `AiBridge` にクローンされ (ブリッジが `ai.ai.character` / `ai.ai.*` を読み取れるようにするため) 、また `Runtime` にもクローンされます (winit イベントループが `Tray::OpenSettings` で `ui.settings_window_visible` を変更できるようにするため)。`std::sync::RwLock` の代わりに `parking_lot` を使用することで、バックグラウンドの書き込みと競合する winit キー押下時のポイズン関連パニックを回避しています。

2. **`AppEvent` は `tokio::sync::mpsc` チャネルであり、`bevy::Message` バスではありません。** レガシーの Bevy アプリは、システム間通信に `bevy::prelude::Messages` を使用していました。v2 には Bevy ECS がないため、プロデューサー・コンシューマーのパターンは、`mpsc::UnboundedSender<AppEvent>` (トレイのスレッドと AI ブリッジの tokio タスクにクローンされる) と、`Runtime` が所有する `UnboundedReceiver<AppEvent>` になります。ドレインは `Runtime::about_to_wait` 内でフレームごとに 1 回発生するため、チャネルは実質的にフレーム境界のイベントバスと同じように機能しますが、Bevy のマクロによるオーバーヘッドはありません。`std::sync::mpsc` ではなく `tokio` の mpsc を選択した理由は、(a) `AiBridge::pump_events` が tokio タスク内で実行され、tokio mpsc がクロス・タスクチャネルとして自然であること、(b) `tokio::sync::mpsc::UnboundedSender` が `Send + Sync` であり、任意の非 GUI スレッド (Windows のトレイスレッドや Linux の GTK スレッドを含む) から `try_send` をサポートしているためです。

3. **トレイは Windows では専用の `GetMessageW` スレッドを使用し、Linux ではメインスレッドでアイコンを構築して別スレッドで `tray-icon` 受信機をポーリングします。** この分割は GTK のスレッドモデルによって強制されています。`gtk` フィーチャーを有効にした `tray-icon` は、`Send` ではない GTK オブジェクト (アイコン、メニュー) を保持するため、メインスレッドに存在する必要があります。Windows にはそのような制約がないため、レガシーコードの「アイコンを所有し、手書きの Win32 メッセージポンプを実行するスレッドを生成する」レシピをそのまま移行しています。Linux スレッドは `TrayIconEvent::receiver()` と `MenuEvent::receiver()` を読み取って、パースしたアクションを `AppEventSender` に転送し、アイコン自体はメインスレッドから外に出ません。

4. **`<|emo:NAME|>` トークンは Bevy の `system` ではなく `AiBridge::pump_events` でパースされます。** レガシーの `character::enqueue_ai_special_tokens` は Bevy のスケジュール上で同じことを行っていました。v2 ではこれをブリッジ内で行います。すべての `EneEvent::SpecialToken` は `ene_core::extract_emotion_from_token(token)` に通され、一致した場合は `AppEvent::EmoteToken(String)` として転送されます。コンシューマー (すなわち `apps/ene-desktop-v2::character::emotion` にある `EmotionQueue`) は PR4 に導入されます。PR1 の役割は単にその経路を配線することです。

5. **フレームごとの `flush_if_dirty` は `Runtime::about_to_wait` に配置されます。** レガシーの `settings_ui::auto_save_config` ran していた処理と同等で、`event_rx` のドレイン後に `state.settings.flush_if_dirty(Some(&char_name))` を 1 回呼び出すことになります。`ConfigStore` は実際の `std::fs::write` をデバウンスし、内部でダーティフラグを追跡するため、毎フレーム呼び出しても安全であり、何か変更がない限りディスクに書き込みません。

6. **CLI は `args[1]=vrm` / `args[2]=vrma` とし、ハードコードされたフォールバックを持ちます。** `AppState::resolve_paths` は `std::env::args().nth(1)` と `.nth(2)` を読み取り、見つからない場合は `DEFAULT_VRM_PATH` / `DEFAULT_VRMA_PATH` にフォールバックします (レガシーと一致)。v2 のみの違いは、2 番目の引数も使用される点です。レガシーの `read_cli_paths` は `(vrm, vrma)` を返していましたが、`main` は `vrm` のみを使用していました (レガシーのコメント：「モーションパスは、検出されたキャラクターの `motion_paths` リストから完全に取得されます」)。v2 はこの動作を踏襲しています。VRM/VRMA のロード機能が実装される PR3 で、明示的な VRMA オーバーライドが追加されます。

### 検証 (PR1 のクローズアウト)

- `cargo build -p ene-desktop-v2` — 成功。
- `cargo clippy -p ene-desktop-v2 -- -D warnings` — 警告なしで成功。開発中に浮上した 23 件の dead-code エラー (コンシューマーが PR2 で導入されるため、新しい公開 API の大部分に `#[allow(dead_code)]` が付与されています) は範囲が限定されており、PR2〜PR5 の導入に伴い削除されます。
- `cargo clippy --workspace --exclude ene-desktop -- -D warnings` — 成功。レガシー `apps/ene-desktop` の clippy エラーは**既存のもの**であり、本スコープ外です (レガシーコード内の実動する `render_settings_window` は「テスト」スタブであるため、`page_ai_page` / `page_character_page` / `page_graphics_page` はデッドコードとして報告されますが、レガシーメンテナーのために残されています)。
- `cargo test --workspace` — 198 パス、6 無視。PR1 では新しいテストは追加されていません (すべての処理は配線であり、最初の動作テストは PR2 の設定 UI とともに導入されます)。
- Windows での手動スモーク: `cargo run -p ene-desktop-v2` を実行するとシステムトレイアイコン「ene」が表示され、左クリックすると UI ウィンドウ (PR0 + PR2 デモ) が開き、トレイメニュー「Settings」が `ui.settings_window_visible` を設定します (PR2 でページが配線されるまでは表示のみのモック)。トレイメニュー「Quit」で正常に終了します。キャラクターウィンドウは依然として PR0 の赤色クアッドを表示します。レガシーの `cargo run -p ene-desktop` は変更されず、Bevy 0.18 ビルドを起動します。
- Linux での手動スモーク (開発機): `Runtime::about_to_wait` 内で GTK ポンプを実行することを除き、同じ動作を示します (フレームごとに 1 回 `gtk::main_iteration_do(false)` を呼び出すだけで、トレイの応答性を維持できます。アイコン自体はメインスレッドを離れません)。

### 既知の制限事項 (PR2+ への持ち越し)

- `AiBridge::run`、`AppEvent::Quit`、`AppEvent::EmoteToken`、および大部分 of `AiStreamUpdate` バリアントには `#[allow(dead_code)]` が設定されています。コンシューマー (設定ページ、ホットキー、ドラッグなど) は PR2〜PR4 で導入されます。
- 設定 UI はまだ PR0 + PR2 のデモ (「Hello from separate egui window!」とクリックカウンター) です。実際の 3 ページ構成の設定 UI は PR2 です。
- システムトレイの「Settings」メニューは `ui.settings_window_visible = true` を設定しますが、PR2 まではそれをレンダリングするものがありません。
- ドラッグ移動とクリックスルーは PR4 / PR5 で導入され、PR1 の v2 キャラクターウィンドウは完全にクリック可能です。
- VRM / VRMA / LookAt / 表情 / スプリングボーンはすべてレガシーの Bevy ビルドに依存しており、v2 のキャラクターウィンドウは赤色クアッドのみを表示します。これらの機能の移行は PR3 / PR4 / PR6 で行われます。

---

## 22.5 PR2 — v2 設定 UI (3 ページ) + ホットキー + キャラクターごとの設定

### モジュールマップ

```
apps/ene-desktop-v2/src/
├── character_state.rs   (新規; 38 行)     AnimationControl, EmotionCommand, EmotionQueue
├── settings.rs          (変更)            +PendingPermission, +PendingUserInput, +QuestionDraft
├── settings_ui/         (新規; 5 ファイル, ~750 行)
│   ├── mod.rs           (PageKind, SettingsUi, apply_egui_visuals)
│   ├── input.rs         (SettingsInputState + sync_from_settings)
│   ├── widgets.rs       (SettingsAction 列挙型, apply_action ディスパッチャ)
│   ├── page_ai.rs       (プロバイダ / キー / 埋め込み / メモリ / チャット / 最新の応答)
│   ├── page_character.rs (キャラクター / モーション / 再生・一時停止 / LookAt / スケール / 位置 / 6つの表情ボタン; Linux: デバッグオーバーレイ、マスクダウンサンプル)
│   └── page_graphics.rs (目標 FPS / シャドウ / AA サイクル行)
├── runtime.rs           (変更)            F1 / WASD / Space / Esc; 表示/非表示; about_to_wait での自動ポップアップ
└── main.rs              (変更)            +mod character_state; +mod settings_ui;
```

### 主要な設計決定

- **Bevy の `Resource` / `Message` / `System` はありません。** ページの描画関数は `(&mut egui::Ui, &mut CharacterSettings, &mut AnimationControl, &Arc<AiBridge>, &mut SettingsInputState, &mut EmotionQueue, f64)` を引数に取ります。これらはランタイムの `about_to_wait` から渡されます。`Arc<AiBridge>` を使用することで、AI ページから直接 `ai.run(&input)` を呼び出すことができます。レガシーの Bevy コードでは、Bevy の `EventWriter` 制約のため `MessageWriter<EneRequestEvent>` を使用していました。
- **F1 is a character-window key, not an egui key.** キャラクターウィンドウの `KeyboardInput` ハンドラが `NamedKey::F1` を検知し、`ui.settings_window_visible` をトグルします。これは UI ウィンドウが開いているか非表示になっているかに関わらず動作し、レガシーのグローバルな `ButtonInput<KeyCode>::just_pressed(KeyCode::F1)` の動作と一致します。
- **WASD ホットキーは `logical_key` (`Key::Character`) ではなく `physical_key` (`KeyCode`) を使用します。** これにより、レガシーの `KeyCode::KeyW` ファミリが持っていた QWERTY → AZERTY のエルゴノミクスが維持されます。ホットキーの割り当ては `cw_char_window_has_focus(cw) && current_page == PageKind::Character` の場合に制限されます。
- **自動ポップアップは2段階で行われます。** `about_to_wait` は、チャネルから保留中の権限/ユーザー入力イベントをローカル of `Option<…>` 累積変数にまず収集し、ループ終了後にそれらを `UiState` に書き込みます。これにより、`self.state.event_rx` を反復処理しながら `self.state.settings.ui.pending_user_input` を可変参照として借用することを回避しています。
- **`EmotionQueue` は `AppState` ではなく `SettingsUi` に保持されます。** キューは UI 側の状態です (ボタンがプッシュし、レンダラがポップします)。ランタイムは単に `UiWindow` の `SettingsUi` から `now_secs = started_at.elapsed().as_secs_f64()` を読み取ります。
- **6 つの表情ボタンは、あえて固定値 `hold_secs: 4.0` を使用しています。** レガシーコードには `character.rs:71` に「将来の拡張機能: `<|DELAY:n|>` トークンで `hold_secs = n` を設定できるようにする」というコメントがありました。v2 でもこの定数を維持し、コメントを `character_state.rs` に移動しています。

### 検証

- `cargo build -p ene-desktop-v2` — 成功。
- `cargo clippy -p ene-desktop-v2 -- -D warnings` — 警告なしで成功。
- `cargo clippy --workspace --exclude ene-desktop -- -D warnings` — 警告なしで成功.
- `cargo test --workspace` — 198 パス、6 無視 (37 スイート)。新しいテストは追加されていません (設定のラウンドトリップテストは、PR3 で VRM フィクスチャとともに導入されます)。
- レガシーの `apps/ene-desktop` (Bevy 0.18) は依然としてビルド可能です (`cargo build -p ene-desktop` — エラーなし、デッドテストスタブ `render_settings_window` による既存の警告が 31 件)。

### 既知の制限事項 (保留)

- 「保留中の権限/質問」ダイアログはまだレンダリングされていませんが、データパスは配線済みです。この領域を処理する次の PR でダイアログが追加されます。
- 数値入力行の `TextEdit` フィールドは表示のみであり、主な入力手段は +/- ボタンです。レガシーコードには Enter キーでの再パース処理がありました。
- `AiBridge::processing` (レガシーの `ene.processing` flag) は、チャット入力をまだ制限していません。PR3 で実装されます。
- システムトレイの「Settings」メニューに、任意のページのフォーカスを渡せるようになりました: `TrayAction::OpenSettings { page: Option<PageKind> }`。トレイのクリックやメニューのパスは依然として `None` を渡しますが (レガシーの動作)、`PermissionRequired` または `UserInputRequired` イベントが到着したときにランタイムが `Some(PageKind::Ai)` をプッシュできるため、ユーザーが操作する前に AI ページ (およびダイアログ) を表示させることができます。

