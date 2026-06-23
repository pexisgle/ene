# `ene-desktop` ECS 移行進捗

このドキュメントは `apps/ene-desktop` を手続き的な `hecs` ベース実装
から `bevy_ecs 0.19` + `bevy_app 0.19` アーキテクチャへ移行する作業
の進捗と計画をまとめたものです。最終的には `bevy_ecs::App` が全フレー
ム処理のロジックを所有し、`hecs` と winit `Runtime` は薄いシェルと
して残ります。

## ステータス

| Phase | スコープ | 状態 |
|------:|-------|------|
| 0 | `bevy_ecs` / `bevy_app` 依存追加、空の `App` スケルトン (`app.rs`, `schedule.rs`)。 | ✅ 完了 |
| 1 | プロセス内のシングルトン (`FrameState`, `ExitRequested`, `TokioHandle`) を bevy `Resource` に昇格。`AppState` は `app: App` を保持し、`Runtime::about_to_wait` が tick。 | ✅ 完了 |
| 2 | 旧 `AppEvent` バスを型付き bevy `Message` に変換。`First` ステージの `pump_legacy_events` システムがレシーバーをドレインしメッセージバッファへ書き込む。 | ✅ 完了 |
| 3 | `CharacterPlugin` — キャラクター単位の components (`VrmModelHandle`, `MotionState`, `SpringBoneState`, `CharacterCamera`, `LookAt`, `EmotionChannel`, `BoneColliders`, `CharacterTransform`, および `Transform` / `GlobalTransform`) を `CharacterBundle` にまとめ、`Startup` でエンティティを生成。 | ✅ 完了 |
| 4 | `PhysicsPlugin` — `PhysicsWorld` をリファクタして `entity_to_*` `HashMap` を削除。`register_character_colliders` は `CharacterColliderRegistration` を返す。ボーン毎のハンドルは `PhysicsBody` / `PhysicsColliders` / `PhysicsColliderStaticOffsets` / `PhysicsColliderStaticRotations` / `PhysicsColliderRestRotations` としてエンティティに保持。`attach_bone_colliders_system` は `Startup`、`step_physics_system` は `Update`。 | ✅ 完了 |
| 5 | `UiPlugin` — 旧 `SettingsUi` 構造体を bevy `Component` (`UiWindow`, `UiPage`, `UiInputDrafts`, `UiAnimation`, `UiEmotionQueue`, `UiStartedAt`, `UiStateComponent`) に分解。`apply_action` は bevy `World` + `Entity` を受け取るようになり、ページ描画関数は `world.get` / `world.get_mut` 経由でコンポーネントを読み書きする。`SettingsActionEvent` メッセージは Phase 6+ での消費に備えて登録済み。 | ✅ 完了 |
| 6 | `PlatformPlugin`, `TrayPlugin`, `AiPlugin` — Phase 6 作業。トレイメニュー、AI ブリッジポンプ、Linux Wayland / X11 の入力領域状態を bevy resource + system へ移行。 | ⏳ 未着手 |
| 7 | レンダリング経路統合 — `acquire` / `encode` / `submit` / `present` を `Last` ステージのシステムに分割。`wgpu::Device` は `NonSend` resource。`CharacterPlugin::finish` が `VrmRenderer` を実体化。 | ⏳ 未着手 |
| 8 | 仕上げ — clippy + test + `Runtime::about_to_wait` を 10 行未満に縮小 (Phase 5 時点: 約 90 行)。 | ⏳ 未着手 |
| 9 | ドキュメント同期 — 英語 (`docs/`) + 日本語 (`docs/ja/`)。 | 🔄 作業中 |

## 主要アーキテクチャルール

* `#[allow(...)]` ではなく `#[expect(...)]` を使用。まだ配線されて
  いないフィールドでも、unfulfilled 状態は明示する。
* bevy の `Message` トレイトには対応する `Messages<T>` リソースが
  必要 — `app.add_message::<T>()` で登録する。
* `PluginGroup::build` は `PluginGroupBuilder` を返す。
  `PluginGroup` 自体は `Default` を derive する。
* SystemSet の構成は `add_systems` ではなく `configure_sets` を使う。
* `app.update()` は 5 ステージ (`First` / `PreUpdate` / `Update` /
  `PostUpdate` / `Last`) と初回のみ `Startup` スケジュールを実行する。
* `IntoScheduleConfigs` は `bevy_ecs::prelude` ではなく
  `bevy_ecs::schedule` に存在する。
* テストワールドでは `MessageWriter` / `MessageReader` で参照する
  各 `T` に対して `world.init_resource::<Messages<T>>()` を呼ぶ。

## ファイル構成 (Phase 5 完了時点)

```text
apps/ene-desktop/src/
├── app.rs                  # DesktopPlugins (PluginGroup) + CorePlugin
├── schedule.rs             # AppSet + configure_schedule / configure_startup
├── component/
│   ├── character.rs        # CharacterBundle + キャラクター components 10 個
│   ├── physics.rs          # PhysicsBody / Colliders / static offsets
│   ├── transform.rs        # Transform / GlobalTransform
│   └── ui.rs               # SettingsUiBundle + UI components 7 個
├── event/
│   ├── ai.rs               # AiTextDelta / AiStreamFinished / AiPermissionRequested / AiUserInputRequested / EmoteToken
│   ├── input.rs            # PointerMoved / PointerButton / KeyboardKey
│   ├── lifecycle.rs        # WindowResized / WindowCloseRequested
│   ├── settings.rs         # OpenSettings
│   └── ui_action.rs        # SettingsActionEvent
├── plugin/
│   ├── character_plugin.rs
│   ├── physics_plugin.rs
│   └── ui_plugin.rs        # UiPlugin + spawn_settings_ui_window
├── resource/
│   ├── event_channels.rs   # EventChannels (legacy ブリッジ)
│   ├── exit.rs             # ExitRequested
│   ├── frame_state.rs      # FrameState
│   ├── pending_actions.rs  # PendingActions (legacy ブリッジ)
│   ├── physics.rs          # PhysicsWorldResource
│   └── tokio.rs            # TokioHandle
├── system/
│   ├── event_pump.rs       # pump_legacy_events
│   └── physics.rs          # attach_bone_colliders_system + step_physics_system
├── settings_ui/            # egui レンダリング (page_character / page_graphics / page_ai / page_debug / widgets / input)
│                            — `apply_action` 系パスは全て bevy `World` / `Entity` 受け取りに変更
└── runtime.rs              # winit + egui グルーコード; 毎フレーム `app.update()` を呼ぶ
```

## 検証結果

* `cargo build -p ene-desktop` — クリーン
* `cargo clippy --workspace -- -D warnings` — クリーン
* `cargo test -p ene-desktop` — 75 passed (Phase 4 72 + 新規 ECS plugin / bundle テスト 3 件)
* `cargo fmt --all` — クリーン

## Phase 6 以降の計画 (残り作業)

### Phase 6 — Platform / Tray / AI

* Linux の `WaylandInputRegionContext` / `LayerShellState` /
  `X11Context` 状態を `PlatformState` から bevy `Resource` へ
  移動。`PlatformPlugin` を追加し、`MaskCaptureState` が `Some`
  のとき `Update` で入力領域を更新するシステムを配線。
* `TrayPlugin` を追加し `TrayHandle` を所有。メニューイベントを
  `OpenSettings { page }` メッセージに変換。
* `AiPlugin` を追加し `AiBridge` と bevy `Resource<PendingAiEvents>`
  を所有。既存の `pump_legacy_events` がレシーバーを `PendingActions`
  にドレインしているので、Phase 6 ではディスパッチャを追加して
  `PendingActions` を既存の `Message` に変換。
* `settings_ui/widgets.rs::apply_action` の 40+ バリアントを
  `system/ui_actions/` 配下の per-action system に分割。各 system
  は 1 つの `SettingsActionEvent` を読み出し、関連する component
  を `UiWindow` エンティティに書き戻す。

### Phase 7 — Render 経路統合

* `WindowPlugin` が `winit::Window` と `wgpu::Surface` を `NonSend`
  resource として所有。
* `RenderPlugin` が `wgpu::Device` / `wgpu::Queue` を `NonSend`
  resource として所有。
* acquire / encode / submit / present を `AppSet::Render` /
  `AppSet::Present` の `Last` ステージシステム化。
* `NonSend<wgpu::Device>` が利用可能になった時点で
  `CharacterPlugin::finish` が `VrmRenderer` を実体化。
* `hecs::World` を最終的に削除。`character_entity` は
  `Query<(Entity, &CharacterRoot)>` で解決。

### Phase 8 — 仕上げ

* `-D warnings` での clippy 通過と unfulfilled `#[expect(dead_code)]`
  をすべて解消。
* `Runtime::about_to_wait` を約 90 行から 10 行未満に縮小 (残るのは
  `EventChannels` リソースへのフレームアクション転送のみ)。
* 新 `SettingsActionEvent` コンシューマシステムの統合テスト。

### Phase 9 — ドキュメント

* 英語版 `docs/architecture/ene-desktop-ecs-migration.md` を最終的な
  `apps/ene-desktop/src/` レイアウトに同期。
* 日本語版 (`docs/ja/architecture/ene-desktop-ecs-migration.md`、本
  ファイル) に翻訳 (本ファイルが source of truth)。
* `docs/applications/desktop.md` (および日本語版) を新 bevy
  アーキテクチャに合わせて更新。
