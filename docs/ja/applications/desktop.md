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
| `AiPlugin` | `ai_bridge.rs` | Bevy ECS と Tokio 間の非同期 AI ストリーミングブリッジ |
| `CharacterPlugin` | `character.rs` | キャラクター生成、表情ブレンド、アニメーション、ヘッドトラッキング |
| `TrayPlugin` | `tray.rs` | システムトレイアイコンとメニュー (Linux/Windows) |
| `SettingsUiPlugin` | `settings_ui/` | egui ベース設定パネル (AI, キャラクター, グラフィック) |
| `CharacterDragPlugin` | `character_drag/` | クリック＆ドラッグウィンドウ移動、透明ヒットテスト |

## AI ブリッジ (`AiPlugin`)

Bevy の同期 ECS ワールドと非同期 `ene-core` ストリーミングエンジンを接続:

```
Bevy ECS (同期)
  → Tokio ランタイム (非同期)
    → run_ai_with_tools()
      → AiStreamEvent パイプライン
        → Bevy イベントとして ECS に戻す
```

システムチェーン:
1. `enqueue_ai_requests` — Bevy UI メッセージ → 内部キュー
2. `process_embedding` — 前フレームからの遅延埋め込み処理
3. `start_next_ai_request` — メモリ初期化 (遅延) → カードロード → 分割タスク → 埋め込み → AI 起動
4. `poll_ai_worker` — ストリームイベントポーリング、表示/サウンド/ツールシステムにディスパッチ

## VRM キャラクターパイプライン

### 表情システム

```
AiStreamEvent::SpecialToken
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
