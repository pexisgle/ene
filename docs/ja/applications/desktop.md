# デスクトップアプリケーション (`ene-desktop`)

VRM キャラクターレンダリングと常時最前面オーバーレイを備えた Bevy ECS ベース GUI アプリ。

## 起動

```bash
cargo run -p ene-desktop
# 特定の VRM を指定:
cargo run -p ene-desktop -- /path/to/character.vrm
# VRM + VRMA アニメーション:
cargo run -p ene-desktop -- /path/to/character.vrm /path/to/animation.vrma
```

## Bevy プラグイン

### コアプラグイン

| プラグイン | 役割 |
|-----------|------|
| `DefaultPlugins` | ウィンドウ、アセット、入力、レンダリング |
| `EguiPlugin` | 設定用イミディエイトモード GUI |
| `VrmPlugin` | VRM 3D モデル読み込み |
| `VrmaPlugin` | VRMA アニメーション読み込みと再生 |

### カスタムプラグイン

| プラグイン | ソース | 役割 |
|-----------|--------|------|
| `ScenePlugin` | `scene.rs` | カメラ、照明、環境、フレーム制限 |
| `EnePlugin` | `ai_bridge.rs` | アクターベース AI ストリーミングブリッジ (EneHandle + Bevy イベント) |
| `CharacterPlugin` | `character.rs` | キャラクター生成、表情ブレンド、アニメーション、ヘッドトラッキング |
| `TrayPlugin` | `tray.rs` | システムトレイアイコンとメニュー (Linux/Windows) |
| `SettingsUiPlugin` | `settings_ui/` | egui ベース設定パネル (AI, キャラクター, グラフィック) |
| `CharacterDragPlugin` | `character_drag/` | クリック＆ドラッグウィンドウ移動、透明ヒットテスト |

## AI ブリッジ (`EnePlugin`)

Bevy の同期 ECS ワールドと非同期アクターベースの `ene-core` を接続:

```
Bevy ECS (同期)
  → EneHandle::run() (mpsc 経由ファイア＆フォーゲット)
    → EneActor (バックグラウンド tokio タスク)
      → EneEvent (broadcast チャンネル)
        → poll_ene_events → EneStreamEvent (Bevy メッセージ)
          → UI / キャラクターシステム
```

### リソース

```rust
#[derive(Resource)]
pub struct EneResource {
    pub handle: EneHandle,    // アクターハンドル
    pub processing: bool,     // AI がストリーミング中かどうか
}
```

### システムチェーン

1. `enqueue_ai_requests` — `EneRequestEvent` を受信 → `handle.run()` を呼び出し
2. `poll_ene_events` — `handle.try_recv()` をループ → `EneStreamEvent` にディスパッチ

**重要な設計:** `handle.try_recv()` を直接使用（`handle.clone()` しない）。毎フレーム `clone()` で broadcast 受信機を再生成すると、新しい受信機は購読時以降のイベントのみ受信するため、イベントがロストする。

### イベント

```rust
pub enum EneStreamEvent {
    TextDelta(String),
    SpecialToken(String),
    ToolCallStart { name: String, arguments: String },
    ToolCallResult { name: String, result: String },
    PermissionRequired { request_id, action, target, description },
    TaskProgress { task_id, step, total_steps, description },
    Finished,
    Error(String),
}
```

## VRM キャラクターパイプライン

### 表情システム

```
EneEvent::SpecialToken
  → poll_ene_events → EneStreamEvent::SpecialToken
  → EmotionQueue (エンキュー)
  → process_emotion_queue (4秒ホールド → フェードアウト)
  → SetExpressions トリガー
  → VRM ブレンドシェイプ値更新
```

### アニメーション再生

VRMA ファイルは事前作成されたアニメーションを提供します。`CharacterPlugin` がアイドル、会話中、感情駆動アニメーション間の再生状態を管理します。

### ヘッドトラッキング

`CharacterDragPlugin` により、キャラクターがマウスカーソル位置を追跡するインタラクティブな「カーソルを見る」効果を実現します。

## ウィンドウプロパティ

| プロパティ | 値 |
|-----------|-----|
| サイズ | 560 × 980 (Windows) |
| スタイル | Windowed (Windows), Borderless Fullscreen (macOS, Linux) |
| Z-order | 常時最前面 |
| 透明度 | コンポジットアルファ (OS 依存) |
| ヒットテスト | 透明領域はクリックスルー (Linux: Wayland layer shell) |

## プラットフォーム対応

| 機能 | Linux (X11) | Linux (Wayland) | Windows |
|------|:---:|:---:|:---:|
| VRM レンダリング | はい (Bevy) | はい (Bevy) | はい (Bevy) |
| 常時最前面 | はい | はい (layer shell) | はい |
| システムトレイ | はい (gtk) | はい (gtk) | はい |
| クリックスルー | はい | はい (input region) | はい |
| ドラッグ移動 | はい | はい (gtk overlay) | はい |
| スクリーンショット | はい | ポータル経由 | はい |
