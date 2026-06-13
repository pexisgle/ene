# `ene-desktop`: Bevy → wgpu / winit / egui 移行プラン

> **ステータス:** 承認済み設計 — 未実装。
> **スコープ:** `apps/ene-desktop` と新設する `crates/ene-vrm` クレート。`ene-core`, `ene-memory`, `ene-cli`, ツールバイナリは変更しない。
> **担当 / 推進:** 未定
> **到達目標:** デスクトップアプリの長年の 2 件の不具合を解消し、我々が完全に所有するスリムで保守性の高い描画スタックを実現する。

---

## 1. 背景と動機

現在の `ene-desktop` は Bevy 0.18 (`bevy_winit`, `bevy_egui`, `bevy_vrm1`, …) の上に構築されている。2 件の具体的なバグにより、サポート対象の 2 つのプラットフォームの少なくとも一方で本番ビルドが使い物にならない。

| # | バグ | トリガー | 影響 |
|---|------|---------|------|
| B1 | egui の入力 / 描画が壊れる | Windows + `WGPU_DX12_PRESENTATION_SYSTEM=DxgiFromVisual`(ウィンドウ単位アルファ取得のため必須) | egui パネルの位置がずれる、クリップ矩形が壊れる、ポインタ追跡が誤る。現状は環境変数を未設定にしてウィンドウ透過がピクセルパーフェクトでないことを許容するワークアラウンド。 |
| B2 | ウィンドウ透過が壊れる | Linux + Vulkan バックエンド (Wayland での唯一の選択肢) | キャラクターウィンドウが完全不透明になり、デスクトップオーバーレイ機能全体が破綻。 |

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

- G1. Windows 10/11: `WGPU_DX12_PRESENTATION_SYSTEM=DxgiFromVisual` を設定した状態で、egui と 3D が透過ウィンドウに正しく合成される。
- G2. Linux + X11 (Vulkan): キャラクターウィンドウは透過、設定ウィンドウは不透明。
- G3. Linux + Wayland (Vulkan、利用可能なら layer-shell): キャラクターウィンドウは透過、コンポジタblur/クリッピングが正しく機能する適切なレイヤを使用。
- G4. `crates/ene-vrm` が glTF / VRM 1.0 を読み込み、スキニング行列を計算し、MToon で描画し、`LookAt` / 表情 API を提供する。
- G5. システムトレイ、ホットキー、設定永続化、AI ブリッジ (`ene-core` の `EneHandle`) はユーザ視点で変化せず動作。
- G6. 既存公開設定スキーマ (`assets/character_settings.schema.json` 等) は不変。

### 2.2 非ゴール (明示的に先送り)

- N1. VRMA 再生と Spring Bone シミュレーション。PR5+ で別管理。
- N2. フレーム単位のシャドウ品質切替 (FXAA / SMAA / TAA トグル)。当面は単一のデフォルトに固定。
- N3. VRM クレートの新規 C-ABI / プラグイン公開面。
- N4. macOS 対応。新コードは macOS でもコンパイル可能だが、ネイティブ依存は用意しない。
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
   - `wgpu::Instance` (Linux は Vulkan、Windows / macOS は既定) と `wgpu::Adapter` + `wgpu::Device` + `wgpu::Queue` を `pollster::block_on` もしくは oneshot で初期化。
   - `winit::EventLoop` を構築し、`ControlFlow::Wait` を設定し、`window_plugin()` を反映する `WindowAttributes` を持つプライマリキャラクターウィンドウ (`Arc<winit::Window>`) を登録: `WindowLevel::AlwaysOnTop`, `transparent: true`, `decorations: false`, `resizable: true`, `inner_size: (320, 480)`。
   - Windows では `Instance::new` の前に `std::env::set_var("WGPU_DX12_PRESENTATION_SYSTEM", "DxgiFromVisual")` を維持。
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

**目的:** キャラクターがカーソルと AI が出力する感情に反応するようにする。

**サブタスク**

1. **表情** — `bevy_vrm1::vrm::expression` を移植:
   - 毎フレーム、最新の `EmotionQueue` (AI ブリッジ駆動) から `BTreeMap<ExpressionName, f32>` を構築。
   - 表情毎の重みをプリミティブ毎のモーフターゲットバッファに乗算。
   - 公開 API: `VrmModel::set_expression(name, weight)`, `VrmModel::expression_names()`。
2. **LookAt** — `bevy_vrm1::vrm::body_tracking` の look-at 部分のみ移植 (当面):
   - 毎フレーム `LookAtTarget { world_position: Vec3 }` を提供。
   - スパイン → ヘッド → 目のチェーンで 2-bone IK を解き、目がターゲットを向くようにする。
   - ヨー / ピッチをモデル定義の VRM クランプ範囲に収める。
   - `apps/ene-desktop/src/character/cursor.rs` で OS カーソル位置をカメラ前方の固定深度のワールド座標に変換して渡す。
3. **BodyTracking** — 最小版のみ: 頭 + 目がカーソルに追従。肩 / 手の揺れは spring bone 再導入まで範囲外。
4. **AI ブリッジ統合** — `AiBridge::drain()` を `runtime` が毎フレーム呼び出し、得られた `EmotionQueue` を `VrmModel::apply_emotions` に供給。

**検証**

- OS カーソルを動かすとモデルの頭 / 目がクランプ角内で追従。
- チャットに "I'm so happy!" を打つと happy ブレンドシェイプへ遷移。
- `ene-vrm` に自動テストを追加: 極小合成 VRM を読み込み、表情を設定し、モーフターゲットバッファがウェイトを反映していることを確認。

### PR5+ — 先送り作業

- **PR5** — VRMA 再生 (`bevy_vrm1::vrma` のクローン)。
- **PR6** — Spring Bone (髪 / 布) — `bevy_vrm1::vrm::spring_bone` の完全移植。
- **PR7** — シャドウ品質切替 (FXAA / SMAA / TAA) と設定 UI ページ追加。
- **PR8** — ドラッグ中の操作性改善 (マルチモニタ処理のスムース化)。

各 PR は個別の設計ドキュメントスニペットとし、PR3 着地時に本ファイル末尾に `## オープンフォローアップ` を追記する。

---

## 5. 新規 / 削除ファイル (一覧)

### 5.1 新規 (トップレベル)

- `crates/ene-vrm/Cargo.toml`
- `crates/ene-vrm/src/lib.rs`
- `crates/ene-vrm/src/loader.rs`
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

### 5.3 削除

- `patches/bevy_winit/` (ディレクトリ丸ごと)
- `apps/ene-desktop/src/scene.rs`
- `apps/ene-desktop/src/character.rs`
- `apps/ene-desktop/src/settings_ui/` (`src/ui/` で置換)
- `apps/ene-desktop/src/character_drag/` (ロジックは `src/platform/drag_subclass.rs` へ)
- `apps/ene-desktop/src/platform.rs` (`src/platform/` に分割)

---

## 6. 依存関係の変更

### 6.1 追加 (ワークスペース)

| クレート | バージョン | 用途 |
|---------|-----------|------|
| `wgpu` | 27 | wgpu コア (Bevy 0.18 と同じ 27 系) |
| `winit` | 0.30 | イベントループとウィンドウ |
| `egui` | 0.33 | イミディエイトモード UI |
| `egui-wgpu` | 0.33 | egui → wgpu レンダラ |
| `egui-winit` | 0.33 | egui 入力統合 |
| `glam` | 0.29 | 線形代数 |
| `gltf` | 1.4 | VRM / glTF パーサ |
| `encase` | 0.12 | シェーダ互換の構造体パッキング (UBO) |
| `bytemuck` | 1 | 安全な `Pod` / `Zeroable` キャスト |
| `pollster` | 0.4 | 起動用の最小 `block_on` |
| `raw-window-handle` | 0.6 | wgpu サーフェス作成 |

### 6.2 維持

- `ene-core`, `ene-memory`, `ene-config`, `ene-provider`, `ene-embedding`, `ene-session`, `ene-tool-*`, `ene-tool-host`, `ene-tool-proto`, `ene-tool-derive`, `ene-common` 関連。
- `tray-icon` (`tray.rs` で直接利用)。
- `tokio`, `serde`, `serde_json`, `figment`, `anyhow`, `thiserror`, `tracing`, `directories`。

### 6.3 削除 (ワークスペース)

- `bevy`, `bevy_ecs`, `bevy_pbr`, `bevy_winit`, `bevy_egui`, `bevy_vrm1`, `bevy_animation`, `bevy_asset`, `bevy_render`, `bevy_math`, `bevy_mesh`, `bevy_window`, `bevy_input`, `bevy_image`, `bevy_transform`, `bevy_utils`。
- `[patch.crates-io] bevy_winit`。

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

- **Windows** — `format = device.adapter.get_supported_surface_formats(...).first()`。`WGPU_DX12_PRESENTATION_SYSTEM=DxgiFromVisual` 下では `CompositeAlphaMode::PreMultiplied` がサポートされ、真のピクセル単位アルファが得られる (B1 解消: 我々がサーフェス / スワップチェイン経路を所有するため)。
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

### 8.4 macOS (コンパイルのみ)

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

- glTF メッシュプリミティブの `targets` フィールドがブレンドシェイプの頂点デルタを保持。
- メッシュ毎に 1 ストレージバッファ: `morph_offsets: array<vec3>` を `[primitive_index][target_index]` で索引。
- 表情名 (例: `joy`, `blink`, `aa`) は `VRMC_vrm-1.0` の `blendShapeMaster` から glTF ターゲットを引き、`ExpressionState` BTreeMap を名前で事前確保。
- 頂点シェーダは `morph_weights: array<vec4>` (4 つ毎にパック) と `morph_offset_count: u32` を取り、`position += Σ(weight[i] * offsets[i])`。

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

