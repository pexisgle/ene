# デスクトップアプリケーション (`ene-desktop`)

VRM キャラクターレンダリングと常時最前面オーバーレイを備えた
winit + `wgpu` + `bevy_ecs 0.19` + `bevy_app 0.19` アプリ。
毎フレームのロジックは `bevy_app::App` が所有し、winit
`Runtime` はそのスケジュールを駆動するだけの薄いレイヤーです。

## 起動

起動オーケストレーションは `apps/ene-desktop/src/startup.rs` に
あります (4 フェーズ — [起動フロー](../architecture/startup.md) を参照)。

```bash
cargo run -p ene-desktop
# 特定の VRM を指定:
cargo run -p ene-desktop -- /path/to/character.vrm
# VRM + VRMA アニメーション:
cargo run -p ene-desktop -- /path/to/character.vrm /path/to/animation.vrma
```

## アーキテクチャ

winit `Runtime` (`apps/ene-desktop/src/runtime.rs`) は
`ApplicationHandler` 実装で、キャラクター / 設定の 2 つの
winit ウィンドウとそれぞれの `wgpu::Surface` を所有します。
`about_to_wait` で毎フレーム bevy スケジュールを実行します:

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
`PostUpdate` / `Last` (初回のみ `Startup` 追加) を実行します。
スケジュール完了後、ランタイムは:

* `ExitRequested` リソースを確認しセットされていればループを
  抜ける
* フレーム毎のカーソル / ヒットテストパイプラインを実行
  (Linux: 入力領域 + クリックスルー、Windows:
  `set_cursor_hittest`)。カーソルのソース・オブ・トゥルースは
  `device_query` で、`update_char_window_cursor_and_hittest` 内で
  読み込まれる。bevy 側 `update_cursor_state_system` は no-op
  スロットで、将来の `PointerMoved` ベース移行用に予約されている
* キャラクターフレームと egui 設定フレームを取得 + エンコード +
  提出 + 表示
* `settings.graphics.target_fps` に基づき
  `set_control_flow(WaitUntil(...))` で次回 winit 起床を予約

`bevy_app::App` は `apps/ene-desktop/src/app.rs` の
`DesktopPlugins` で構成されます:

| プラグイン | ソース | 役割 |
|-----------|--------|------|
| `CorePlugin` | `app.rs` | `FrameState`, `ExitRequested`, `TokioHandle`, `EventChannels` (レガシーブリッジ)、13 種類の `Message` 型を登録。 |
| `CharacterPlugin` | `plugin/character_plugin.rs` | `Startup` で `CharacterBundle` エンティティを生成。 |
| `PhysicsPlugin` | `plugin/physics_plugin.rs` | `Startup` で `attach_bone_colliders_system`、`Update` で `step_physics_system`。 |
| `UiPlugin` | `plugin/ui_plugin.rs` | `Startup` で `SettingsUiBundle` エンティティを生成。 |
| `PlatformPlugin` | `plugin/platform_plugin.rs` | フレーム毎のカーソル状態、入力領域更新、クリックスルー。 |
| `TrayPlugin` | `plugin/tray_plugin.rs` | Linux 限定の `tick_gtk_system` を `Last` に追加 (drain 専用、アイコンロジックなし)。 |
| `AiPlugin` | `plugin/ai_plugin.rs` | `system::ui_consumers` 5 つのシステムを `Update` に追加。 |

スケジュール (`apps/ene-desktop/src/schedule.rs`) は 6 つの
セットから成ります:

* `EventDispatch` (`First` 内) — `pump_legacy_events` が
  レガシー `AppEvent` レシーバーを型付き bevy `Message` に
  ドレイン。
* `Input` — `should_render_debug_system` (ドラッグ / デバッグ
  FPS ゲート)。`update_cursor_state_system` も `Input` 内に登録
  されているが、意図的に no-op スロットとして将来の
  `PointerMoved` ベースカーソルソース移行用に予約されている。
* `Settings` — `apply_linux_click_through_system`,
  `refresh_input_region_system`, `open_settings_system`,
  `apply_ai_text_deltas_system`, `apply_ai_permission_system`,
  `apply_ai_user_input_system`, `apply_emotions_system`,
  `apply_settings_action_system`。
* `Animation` — `step_physics_system`。
* `Render` / `Present` — プレースホルダ (実際の GPU 提出は
  `CharacterRenderer` が `!Send + !Sync` であるため
  `Runtime::render_per_frame` 側で実行)。

## AI ブリッジ

デスクトップアプリは upstream の `ene-runtime` アクター
(`EneHandle` / `EneEvent`) を薄い `AiBridge` シム
(`apps/ene-desktop/src/ai_bridge.rs`) 経由で使います。
ブリッジは:

1. 現在の tokio ランタイム上で `EneHandle::open` により準備済みハンドルを開き、
   ブロードキャスト `EneEvent` ストリームを購読。
2. `EneEvent` → `AppEvent` (`AiStreamUpdate`, `PerformanceCue` / `EmoteToken`,
   `StatusChanged`, …) にマップして
   クロスサブシステム `AppEventSender` に push するバックグラ
   ウンドドレインタスクを spawn。
3. `processing: Arc<AtomicBool>` フラグを所有
   (`run` でセット、`Terminal` でクリア)。

ユーザー入力は `AiBridge::run` / `AiBridge::cancel` 経由で
返送されます（ターン範囲。`cancel` はアクティブな `TurnId` を取る）。

`EventChannels` (bevy `Resource`) は `AppEvent` バスの
レシーバー側を保持します。`system::event_pump::pump_legacy_events`
が `First` / `EventDispatch` でドレインし、型付き
`Message` (`AiTextDelta`, `AiPermissionRequested`,
`AiUserInputRequested`, `AiStreamFinished`, `EmoteToken`, …)
に書き込みます。それらを `system::ui_consumers` 配下の
システムが読みます。

```
tokio EneActor (ene-runtime)
  → EneEvent (broadcast)
    → AiBridge バックグラウンドタスク
      → AppEvent (mpsc)
        → EventChannels.rx
          → pump_legacy_events (First/EventDispatch)
            → Messages<AiTextDelta> / Messages<AiPermissionRequested> / …
              → apply_ai_text_deltas_system / apply_ai_permission_system / … (Update)
                → UiStateComponent / CharacterSettings
```

### メッセージ型

`CorePlugin` が登録する 13 種類のメッセージ:

| メッセージ | 生成元 | 消費側 |
|-----------|--------|--------|
| `AiTextDelta` | `pump_legacy_events` | `apply_ai_text_deltas_system` |
| `AiStreamFinished` | `pump_legacy_events` | (AI ページ自身が消費) |
| `AiPermissionRequested` | `pump_legacy_events` | `apply_ai_permission_system` |
| `AiUserInputRequested` | `pump_legacy_events` | `apply_ai_user_input_system` |
| `EmoteToken` | `pump_legacy_events` | `apply_emotions_system` |
| `PointerMoved` | `pump_window_events` | `update_cursor_state_system` (no-op; `device_query` がカーソルのソース・オブ・トゥルース) |
| `PointerButton` | `pump_window_events` | (ドラッグ / 将来のシステム) |
| `KeyboardKey` | `pump_window_events` | (設定ホットキー 将来用) |
| `WindowResized` | `pump_window_events` | (リサイズハンドラ) |
| `WindowCloseRequested` | `pump_window_events` | (ループ終了) |
| `OpenSettings` | `system::ui_dispatcher` | `open_settings_system` |
| `SettingsActionEvent` | `system::ui_dispatcher` | `apply_settings_action_system` (drain 専用プレースホルダ) |
| `TickGtk` | `pump_legacy_events` (Linux) | `tray_tick::tick_gtk_system` (drain 専用) |

## ウィンドウプロパティ

| プロパティ | 値 |
|-----------|-----|
| キャラクターサイズ | `settings.graphics.character_size` に追従 |
| UI サイズ | 460 × 620 (設定ウィンドウ) |
| Z-order | 常時最前面 |
| 透明度 | コンポジットアルファ (OS 依存) |
| ヒットテスト | 透明領域はクリックスルー (Linux: Wayland `set_input_region` + X11 `shape::rectangles`; Windows: `WS_EX_TRANSPARENT`) |

## プラットフォーム対応

| 機能 | Linux (X11) | Linux (Wayland) | Windows |
|------|:---:|:---:|:---:|
| VRM レンダリング | はい (wgpu) | はい (wgpu) | はい (wgpu) |
| 常時最前面 | はい | はい (layer shell) | はい |
| システムトレイ | はい (gtk) | はい (gtk) | はい |
| クリックスルー | はい (`shape` ext) | はい (`set_input_region`) | はい (`WS_EX_TRANSPARENT`) |
| ドラッグ移動 | はい | はい | はい |
| スクリーンショット | はい | ポータル経由 | はい |

## ファイル構成

プラグイン順序と ECS リソース構成は `apps/ene-desktop/src/plugin/` および `apps/ene-desktop/src/resource/` にあります。レンダーパスは `CharacterRenderer` と wgpu 型が `!Send + !Sync` のため bevy システム外に残しています — [起動フロー](../architecture/startup.md) を参照。
