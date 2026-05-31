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
            EnePlugin,          # EneHandle アクターによる AI ストリーミング
            CharacterPlugin,    # 表情、アニメーション、ヘッドトラッキング
            TrayPlugin,         # システムトレイ
            SettingsUiPlugin,   # egui 設定パネル
            CharacterDragPlugin,# クリック＆ドラッグ移動
        ))
        .run()
```

### AI 統合 (`EnePlugin`)

アクターは Bevy `Resource` として初期化:

```rust
#[derive(Resource)]
pub struct EneResource {
    pub handle: EneHandle,    // アクターハンドル — コマンド送信、イベント受信
    pub receiver: EneEventReceiver,  // ブロードキャスト受信機 — イベント受信
    pub processing: bool,     // AI ストリームがアクティブかどうか
}
```

Bevy システムチェーン:
1. `enqueue_ai_requests` — Bevy の `EneRequestEvent` メッセージ → `handle.run()` (ファイア＆フォーゲット)
2. `poll_ene_events` — `receiver.try_recv()` をループ → `EneStreamEvent` メッセージにディスパッチ

イベントフロー: `EneEvent` (broadcast) → `poll_ene_events` → `EneStreamEvent` (Bevy メッセージ) → UI/キャラクターシステム

**重要:** `poll_ene_events` は `ene.receiver.try_recv()` を直接使用（`handle.subscribe()` しない）。毎フレーム `subscribe()` で broadcast 受信機を再生成すると、新しい受信機は購読時以降のイベントのみ受信するため、イベントがロストする。

### ウィンドウプロパティ

| プロパティ | 値 |
|-----------|-----|
| サイズ | 560 × 980 (Windows) |
| スタイル | Windowed (Windows) / Borderless Fullscreen (macOS, Linux) |
| Z-order | 常時最前面 |
| 透明度 | コンポジットアルファ (OS 依存) |

### 表情適用

```
EneEvent::SpecialToken → poll_ene_events → EneStreamEvent::SpecialToken
  → EmotionQueue → 4秒ホールド + フェードアウト
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
  │   └── EneHandle::new() → アクターを生成
  ├── --tooltest → tooltest::run() → 終了
  └── 通常モード:
      ├── AppContext { handle: EneHandle }
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
| `/clear` | 履歴クリア |
| `/prompt` | システムプロンプト表示 |
| `/card <path>` | キャラクターカード変更 (非同期) |
| `/config` | 設定表示 |
| `/tools` | 有効なツール一覧 |
| `/history` | 会話履歴表示 |
| `/undo` | 最後のファイル操作を取り消し |
| `/tooltest [prompt]` | ワンショットツールテスト |
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
