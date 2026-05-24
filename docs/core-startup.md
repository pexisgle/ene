# アプリケーション起動フロー

## GUI（ene-desktop）

Bevy ECS ベースのデスクトップアプリ。VRM キャラクターを常時最前面表示する。

### 起動シーケンス

```
main()
  ├── resources::ensure_resource_dirs()  # アセットディレクトリ作成
  ├── read_cli_paths()                   # VRM/VRMA パス取得（CLI引数 or デフォルト）
  ├── CharacterSettings::discover()      # キャラクタースキャン＋設定読み込み
  └── App::new()
        .insert_resource(settings)
        .add_plugins((
            DefaultPlugins.set(window_plugin()),  # 透明・常時最前面ウィンドウ
            EguiPlugin,         # 設定UI
            VrmPlugin,          # VRM 読み込み
            VrmaPlugin,         # VRMA アニメーション
            ScenePlugin,        # カメラ・ライト・フレーム制限
            AiPlugin,           # AI ストリーミング統合
            CharacterPlugin,    # キャラクター生成・表情・アニメーション
            TrayPlugin,         # システムトレイ
            SettingsUiPlugin,   # egui 設定ウィンドウ
            CharacterDragPlugin,# キャラクタークリック＆ドラッグ
        ))
        .run()
```

### AI 統合（AiPlugin）

Bevy システムチェーン:

1. `enqueue_ai_requests`: Bevy メッセージ → キュー
2. `process_embedding`: 埋め込み処理（前フレームからの継続）
3. `start_next_ai_request`: メモリ初期化（遅延実行）→ カードロード → セッション分割タスク起動 → 埋め込み → `run_ai_with_tools` 起動
4. `poll_ai_worker`: ストリームイベント受信 → TextDelta/SpecialToken/ToolCall 処理 → Finished で完了

メモリ初期化は初回 AI リクエスト時に遅延実行される。

### ウィンドウ特性

| 項目 | 値 |
|------|-----|
| サイズ | 560x980（Windows） |
| スタイル | Windows: Windowed / macOS・Linux: BorderlessFullscreen |
| Z-order | 常時最前面（AlwaysOnTop） |
| 透明度 | CompositeAlphaMode（OS依存） |

### 表情適用フロー

```
AiStreamEvent::SpecialToken
  → EmotionQueue にエンキュー（enqueue_ai_special_tokens）
  → process_emotion_queue が4秒ホールド後フェードアウト
  → SetExpressions トリガーで VRM blendshape 反映
```

## CLI（ene-cli）

`#[tokio::main]` ベースのインタラクティブ REPL。

### 起動シーケンス

```
main()
  ├── clap: Args 解析（--tooltest フラグ）
  ├── config::init()
  │   ├── ensure_resource_dirs()
  │   ├── settings.json 読み込み（ene_config::load_settings）
  │   └── AiRuntime::init(settings) で session / registry を初期化
  ├── --tooltest 指定時 → tooltest::run() で終了
  └── 通常モード:
      ├── registry::build(settings) → ToolHostManager 起動＋MCP接続
      └── repl::run(ctx) → 対話ループ開始
```

### REPL ループ

1. `dialoguer::Input` でプロンプト表示
2. `/` コマンドは `commands::execute()` で処理

| コマンド | 機能 |
|----------|------|
| `/quit` | 終了 |
| `/clear` | 履歴クリア |
| `/prompt` | システムプロンプト表示 |
| `/card <path>` | キャラクターカード変更 |
| `/config` | 設定表示 |
| `/tools` | ツール一覧 |
| `/history` | 会話履歴表示 |
| `/undo` | Undo 実行 |
| `/tooltest [prompt]` | ワンショットツールテスト |
| `/memory search <query>` | 記憶検索 |
| `/memory list` | 保存済み要約/キーファクト一覧 |
| `/session split` | 手動セッション分割 |
| `/session info` | セッション情報 |
| `/session summaries` | 過去要約一覧 |
| `/help` | ヘルプ表示 |

3. 通常入力時:
   - `poll_split_result` で分割完了確認
   - `check_and_perform_split` で境界チェック起動
   - ユーザー入力を埋め込み
   - `run_ai_with_tools` 実行
   - `process_stream` でイベント処理・表示

### ストリーム表示

| イベント | 表示 |
|----------|------|
| TextDelta | 標準出力（flush） |
| SpecialToken（emo） | `[Emotion: name]` マゼンタ |
| ToolCallStart | `[Tool Calling: name(args)]` シアン |
| ToolCallResult | `[Tool Result: result]` 緑 |
| SessionSplit | 理由＋要約 黄色 |
| Error | 赤字 |
