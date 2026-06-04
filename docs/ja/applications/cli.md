# CLI リファレンス (`ene-cli`)

AI キャラクターとの対話、ツールテスト、メモリ/セッション管理のための対話型 REPL。

## 起動

```bash
cargo run -p ene-cli
```

## アーキテクチャ

```
main.rs → clap 引数解析
  → config::init() → 設定読み込み, EneHandle::new()
  → AppContext { handle: EneHandle, commands: Vec<Arc<dyn CliCommand>> }
  → repl::run() → dialoguer 入力ループ
      → stream::process_stream() → EneEvent バリアント処理
      → commands::execute() → CliCommand トレイト経由の / コマンドディスパッチ
```

CLI は起動時に `EneHandle`（アクター）を作成。ユーザー入力は `handle.run()` で送信し、イベントは `handle.subscribe()` で受信。

### CliCommand トレイト

各 `/` コマンドは `CliCommand` トレイトを実装します:

```rust
#[async_trait]
pub trait CliCommand: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn usage(&self) -> &'static str;
    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), String>;
}
```

コマンドは `COMMANDS` スライスに登録され、名前でディスパッチされます。

## REPL コマンド

コマンドは `/` プレフィックスで入力:

### セッションコマンド

| コマンド | 動作 |
|---------|------|
| `/quit` | REPL を終了 |
| `/clear` | 会話履歴をクリア |
| `/history` | 会話履歴を表示 |
| `/prompt` | 現在のシステムプロンプトを表示 (system, examples, memory, expression protocol) |

### キャラクターコマンド

| コマンド | 動作 |
|---------|------|
| `/card <path>` | 別のキャラクターカードを読み込み (非同期) |

### 設定とツール

| コマンド | 動作 |
|---------|------|
| `/config` | 現在の設定を表示 (provider, model, embedding, memory) |
| `/tool list` | 登録済みの全ツールを一覧 |
| `/tool help <name>` | ツールの詳細ヘルプを表示 |
| `/tool call <name> <json>` | JSON 引数でツールを直接呼び出し |
| `/undo` | プレースホルダー (アクターベースランタイムでは未対応) |

### メモリコマンド

| コマンド | 動作 |
|---------|------|
| `/memory search <query>` | 長期記憶を検索 (埋め込み類似度) |
| `/memory list` | 保存済み要約とキーファクトを一覧 |

### セッション分割コマンド

| コマンド | 動作 |
|---------|------|
| `/session split` | 手動でセッション分割を実行 (アクターの ManualSplit コマンド経由) |
| `/session info` | セッション診断情報を表示 |
| `/session summaries` | 過去のセッション要約を一覧 |

### ヘルプ

| コマンド | 動作 |
|---------|------|
| `/help` | 利用可能なコマンドを表示 |

## ストリーム表示

| イベント | スタイル |
|---------|---------|
| 通常テキスト | デフォルト stdout |
| `[Emotion: happy]` | マゼンタ |
| `[Tool Calling: name(args)]` | シアン |
| `[Tool Result: ...]` | 緑 |
| `[Session split]` | 黄色 |
| エラー | 赤太字 |