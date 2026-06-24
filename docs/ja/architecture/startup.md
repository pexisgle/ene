# アプリケーション起動フロー

## デスクトップ (ene-desktop)

`ene-desktop` は `winit` + `wgpu` + `egui` シェルです。
フレーム単位のロジックは `bevy_app::App` が所有し、winit
イベントループがそのスケジューラを駆動します。**Bevy
プラグインや Bevy レンダラーは使用しません** — スケジューラ
として `bevy_ecs 0.19` / `bevy_app 0.19` を、VRM レンダリング
として `ene-vrm` を使用します。

### 起動シーケンス

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"));
    fmt().with_env_filter(filter).init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let handle = runtime.handle().clone();
    let _guard = runtime.enter();

    let (assets_dir, default_vrm, _default_vrma) = state::resolve_paths()?;
    let gpu = pollster::block_on(gpu::GpuContext::new())?;
    let settings = settings::CharacterSettings::discover(&assets_dir, default_vrm);
    let (app_state, event_tx) = state::AppState::with_channel(gpu, settings, &handle);

    let event_loop = winit::event_loop::EventLoop::new()?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);

    let mut app = runtime::Runtime::new(app_state, event_tx);
    event_loop.run_app(&mut app).expect("winit event loop failed");

    drop(_guard);
    runtime.shutdown_timeout(std::time::Duration::from_millis(500));
    Ok(())
}
```

### `winit` → `bevy_app` ブリッジ

winit `Runtime` (`apps/ene-desktop/src/runtime.rs`) は
`ApplicationHandler` を実装し、2 つの winit ウィンドウ
(キャラクター + 設定) とそれぞれの `wgpu::Surface` を所有
します。`about_to_wait` ごとに完全な bevy スケジュールを
実行します:

```rust
fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
    self.sync_runtime_to_bevy();
    self.state.app.update();
    if self.handle_exit(event_loop) { return; }
    self.run_debug_pipeline();
    self.render_per_frame(event_loop);
    self.set_frame_deadline(event_loop);
}
```

`app.update()` は `First` / `PreUpdate` / `Update` /
`PostUpdate` / `Last` (初回は `Startup` を含む) を実行します。
スケジュール終了後、ランタイムは:

- `ExitRequested` リソースを確認し、設定されていればループ
  を終了;
- フレーム毎のカーソル / ヒットテストパイプラインを実行
  (Linux: 入力領域 + クリックスルー、Windows:
  `set_cursor_hittest`);
- キャラクターフレームと egui 設定フレームを取得 + エンコード
  + 表示;
- 次回の winit ウェイクアップを
  `set_control_flow(WaitUntil(...))` でスケジュール
  (基準: `settings.graphics.target_fps`)。

### Bevy プラグイン (`DesktopPlugins`)

bevy `App` は `apps/ene-desktop/src/app.rs` で構成されます:

| プラグイン | ソース | 役割 |
|----------|--------|------|
| `CorePlugin` | `app.rs` | `FrameState`, `ExitRequested`, `TokioHandle`, `EventChannels` (レガシーブリッジ), 13 種類の `Message` 型 |
| `CharacterPlugin` | `plugin/character_plugin.rs` | `Startup` で `CharacterBundle` エンティティを生成 |
| `PhysicsPlugin` | `plugin/physics_plugin.rs` | `Startup` で `attach_bone_colliders_system`; `Update` で `step_physics_system` |
| `UiPlugin` | `plugin/ui_plugin.rs` | `Startup` で `SettingsUiBundle` エンティティを生成 |
| `PlatformPlugin` | `plugin/platform_plugin.rs` | フレーム毎のカーソル状態、入力領域リフレッシュ、クリックスルー |
| `TrayPlugin` | `plugin/tray_plugin.rs` | Linux 専用の `tick_gtk_system` (ドレイン専用) |
| `AiPlugin` | `plugin/ai_plugin.rs` | `Update` で `system::ui_consumers` システム群を追加 |

スケジュール (`apps/ene-desktop/src/schedule.rs`) は 6 つの
セットを持ちます:

- `EventDispatch` (`First` 内) — `pump_legacy_events` が
  レガシー `AppEvent` 受信機を型付き bevy `Message` にドレイン
  する。
- `Input` — `should_render_debug_system` (ドラッグ /
  デバッグ FPS ゲート)。`update_cursor_state_system` も
  `Input` にあるが、意図的に no-op (将来の
  `PointerMoved` ベースカーソル用のスロット)。
- `Settings` — `apply_linux_click_through_system`,
  `refresh_input_region_system`, `open_settings_system`,
  `apply_ai_text_deltas_system`, `apply_ai_permission_system`,
  `apply_ai_user_input_system`, `apply_emotions_system`,
  `apply_settings_action_system`。
- `Animation` — `step_physics_system`。
- `Render` / `Present` — プレースホルダーセット。実際の GPU
  サブミッションは `Runtime::render_per_frame` で実行
  (`CharacterRenderer` が `!Send + !Sync` であるため)。

### AI 統合 (`AiBridge`)

`ene-desktop` は `ene-core` アクター (`EneHandle` /
`EneEvent`) を薄い shim (`apps/ene-desktop/src/ai_bridge.rs`)
経由で消費します。ブリッジは:

1. 現在の tokio ランタイム上で `EneHandle` を生成し、
   broadcast `EneEvent` ストリームを購読する。
2. バックグラウンドのドレインタスクを生成し、`EneEvent` →
   `AppEvent` (`AiStreamUpdate`, `EmoteToken`,
   `SessionSplit`, `StatusChanged` 等) にマッピングして
   クロスサブシステムの `AppEventSender` に push する。
3. `processing: Arc<AtomicBool>` フラグを所有し、
   `EneCommand::Run` でセット、`Done` / `Failed` で
   クリアする。

ユーザー入力は `AiBridge::run` / `AiBridge::cancel` (fire-and-forget
mpsc 送信) 経由で逆流する。

`EventChannels` (bevy `Resource`) は `AppEvent` バスの受信機
半分を保持する。`system::event_pump::pump_legacy_events` が
`First` / `EventDispatch` でドレインし、型付き `Message`
(`AiTextDelta`, `AiPermissionRequested`,
`AiUserInputRequested`, `AiStreamFinished`, `EmoteToken` 等) を
書き出す。フレーム毎の `system::ui_consumers` システムが
それを読む。

```
tokio EneActor (ene-core)
  → EneEvent (broadcast)
    → AiBridge バックグラウンドタスク
      → AppEvent (mpsc)
        → EventChannels.rx
          → pump_legacy_events (First/EventDispatch)
            → Messages<AiTextDelta> / Messages<AiPermissionRequested> / …
              → apply_ai_text_deltas_system / apply_ai_permission_system / … (Update)
                → UiStateComponent / CharacterSettings
```

#### `CorePlugin` が登録する 13 種類の `Message`

| Message | ソース | コンシューマー |
|---------|--------|---------------|
| `AiTextDelta` | `pump_legacy_events` | `apply_ai_text_deltas_system` |
| `AiStreamFinished` | `pump_legacy_events` | (AI ページ自身が消費) |
| `AiPermissionRequested` | `pump_legacy_events` | `apply_ai_permission_system` |
| `AiUserInputRequested` | `pump_legacy_events` | `apply_ai_user_input_system` |
| `EmoteToken` | `pump_legacy_events` | `apply_emotions_system` |
| `PointerMoved` | `pump_window_events` | `update_cursor_state_system` (no-op; カーソルは `device_query` が真) |
| `PointerButton` | `pump_window_events` | (ドラッグ / 将来用) |
| `KeyboardKey` | `pump_window_events` | (設定ホットキー用) |
| `WindowResized` | `pump_window_events` | (リサイズハンドラ) |
| `WindowCloseRequested` | `pump_window_events` | (ループを終了) |
| `OpenSettings` | `system::ui_dispatcher` | `open_settings_system` |
| `SettingsActionEvent` | `system::ui_dispatcher` | `apply_settings_action_system` (ドレイン専用) |
| `TickGtk` | `pump_legacy_events` (Linux) | `tray_tick::tick_gtk_system` (ドレイン専用) |

### ウィンドウプロパティ

| プロパティ | 値 |
|----------|-----|
| キャラクターサイズ | `settings.graphics.character_size` に従う |
| UI サイズ | 460 × 620 (設定ウィンドウ) |
| Z-order | 常時最前面 |
| 透明度 | コンポジットアルファ (OS 依存) |
| ヒットテスト | 透明領域はクリックスルー (Linux: Wayland `set_input_region` + X11 `shape::rectangles`; Windows: `WS_EX_TRANSPARENT`) |

### プラットフォーム対応

| 機能 | Linux (X11) | Linux (Wayland) | Windows |
|------|:---:|:---:|:---:|
| VRM レンダリング | 可 (wgpu) | 可 (wgpu) | 可 (wgpu) |
| 常時最前面 | 可 | 可 (layer shell) | 可 |
| システムトレイ | 可 (gtk) | 可 (gtk) | 可 |
| クリックスルー | 可 (`shape` 拡張) | 可 (`set_input_region`) | 可 (`WS_EX_TRANSPARENT`) |
| ドラッグ移動 | 可 | 可 | 可 |
| スクリーンショット | 可 | ポータル経由 | 可 |

### 表情適用

```
EneEvent::SpecialToken → AiBridge → AppEvent::EmoteToken
  → pump_legacy_events → Message<EmoteToken>
    → apply_emotions_system (4秒ホールド + フェードアウト)
      → SetExpressions → VRM ブレンドシェイプ更新 (ene-vrm)
```

詳細は
[`docs/architecture/ene-desktop-ecs-migration.md`](ene-desktop-ecs-migration.md)
および [`docs/applications/desktop.md`](../applications/desktop.md)
を参照。

---

## CLI (ene-cli)

`#[tokio::main]` 対話型 REPL。

### 起動シーケンス

```
main()
  ├── clap: Args 解析
  ├── config::init()
  │   ├── ensure_resource_dirs()
  │   ├── settings.json 読み込み
  │   └── EneHandle::new() → アクターを生成
  └── 通常モード:
      ├── AppContext { handle: EneHandle, commands: Vec<Arc<dyn CliCommand>> }
      └── repl::run(ctx) → 対話ループ
```

### REPL ループ

1. `dialoguer::Input` でプロンプト表示
2. `/` コマンドは `commands::execute()` で処理
3. 通常入力: `handle.run()` + `process_stream()` でイベント表示

**イベントサブスクリプションパターン:**
```rust
let mut rx = ctx.handle.subscribe();  // コマンド送信前に受信機を取得
ctx.handle.run(&input);               // Run コマンドを送信
stream::process_stream(&mut rx, &ctx.handle).await;  // イベントを処理
```

これにより `run()` 呼び出しと最初の `recv()` の間のイベントロストを防ぐ。

### REPL コマンド

| コマンド | 動作 |
|---------|------|
| `/quit` | REPL を終了 |
| `/clear` | 次回実行時に履歴がリフレッシュされることを示す (手動クリアは no-op) |
| `/prompt` | システムプロンプト表示 |
| `/card <path>` | キャラクターカード変更 (非同期) |
| `/config` | 設定表示 |
| `/tool list` | 登録済みツール一覧 |
| `/tool help <name>` | ツールの詳細ヘルプ表示 |
| `/tool call <name> <json>` | ツールの直接呼び出し |
| `/history` | 会話履歴表示 |
| `/undo` | プレースホルダー (アクターベースランタイムでは未対応) |
| `/memory search <q>` | 記憶検索 |
| `/memory list` | 保存済み要約/ファクト一覧 |
| `/session split` | 手動セッション分割 (ManualSplit コマンド経由) |
| `/session info` | セッション診断 |
| `/session summaries` | 過去の要約一覧 |
| `/help` | ヘルプ |

### ストリーム表示書式

| イベント | 出力スタイル |
|---------|-------------|
| テキスト | デフォルト stdout |
| `SpecialToken(emo)` | `[Emotion: name]` マゼンタ |
| `ToolCallStart` | `[Tool Calling: name(args)]` シアン |
| `ToolCallResult` | `[Tool Result: ...]` 緑 |
| `SessionSplit` | 理由 + 要約 黄色 |
| `Error` | 赤字 |
