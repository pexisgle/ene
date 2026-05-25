# アプリケーション起動フロー

## デスクトップ (ene-desktop)

Bevy ECS ベースのアプリケーション。VRM キャラクターを常時最前面に表示します。

### 起動シーケンス

```
main()
  ├── resources::ensure_resource_dirs()
  ├── read_cli_paths()                   # CLI 引数またはデフォルトから VRM/VRMA パス取得
  ├── CharacterSettings::discover()      # キャラクタースキャン + 設定読み込み
  └── App::new()
        .insert_resource(settings)
        .add_plugins((
            DefaultPlugins.set(window_plugin()),  # 透明・常時最前面ウィンドウ
            EguiPlugin,         # 設定 UI
            VrmPlugin,          # VRM モデル読み込み
            VrmaPlugin,         # VRMA アニメーション
            ScenePlugin,        # カメラ、ライト、フレーム制限
            AiPlugin,           # AI ストリーミング統合
            CharacterPlugin,    # 表情、アニメーション、ヘッドトラッキング
            TrayPlugin,         # システムトレイ
            SettingsUiPlugin,   # egui 設定パネル
            CharacterDragPlugin,# クリック＆ドラッグ移動
        ))
        .run()
```

### AI 統合 (`AiPlugin`)

Bevy システムチェーン:
1. `enqueue_ai_requests` — Bevy メッセージ → 内部キュー
2. `process_embedding` — バックグラウンド埋め込み計算
3. `start_next_ai_request` — 遅延メモリ初期化 → カードロード → 分割タスク生成 → 埋め込み → `run_ai_with_tools`
4. `poll_ai_worker` — ストリームイベント消費 → 表示/サウンド/ツール処理

メモリ初期化は最初の AI リクエストまで遅延されます。

### ウィンドウプロパティ

| プロパティ | 値 |
|-----------|-----|
| サイズ | 560 × 980 (Windows) |
| スタイル | Windowed (Windows) / Borderless Fullscreen (macOS, Linux) |
| Z-order | 常時最前面 |
| 透明度 | コンポジットアルファ (OS 依存) |

### 表情適用

```
AiStreamEvent::SpecialToken → EmotionQueue → 4秒ホールド + フェードアウト
    → SetExpressions → VRM ブレンドシェイプ更新
```

---

## CLI (ene-cli)

`#[tokio::main]` 対話型 REPL。

### 起動シーケンス

```
main()
  ├── clap: Args 解析 (--tooltest フラグ)
  ├── config::init()
  │   ├── ensure_resource_dirs()
  │   ├── settings.json 読み込み
  │   └── AiRuntime::init(settings) → セッション + ツールホスト
  ├── --tooltest → tooltest::run() → 終了
  └── 通常モード:
      ├── ツールレジストリ構築 (ToolHostManager.start + MCP 接続)
      └── repl::run(ctx) → 対話ループ
```

### REPL ループ

1. `dialoguer::Input` でプロンプト表示
2. `/` コマンドは `commands::execute()` で処理

| コマンド | 動作 |
|---------|------|
| `/quit` | 終了 |
| `/clear` | 履歴クリア |
| `/prompt` | システムプロンプト表示 |
| `/card <path>` | キャラクターカード変更 |
| `/config` | 設定表示 |
| `/tools` | 有効なツール一覧 |
| `/history` | 会話履歴表示 |
| `/undo` | 最後のファイル操作を取り消し |
| `/tooltest [prompt]` | ワンショットツールテスト |
| `/memory search <q>` | 記憶検索 |
| `/memory list` | 保存済み要約/ファクト一覧 |
| `/session split` | 手動セッション分割 |
| `/session info` | セッション診断 |
| `/session summaries` | 過去の要約一覧 |
| `/help` | ヘルプ |

### ストリーム表示書式

| イベント | 出力スタイル |
|---------|-------------|
| `TextDelta` | stdout (flush) |
| `SpecialToken(emo)` | `[Emotion: name]` マゼンタ |
| `ToolCallStart` | `[Tool Calling: name(args)]` シアン |
| `ToolCallResult` | `[Tool Result: ...]` 緑 |
| `SessionSplit` | 理由 + 要約 黄色 |
| `Error` | 赤字 |
