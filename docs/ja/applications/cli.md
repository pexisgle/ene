# CLI リファレンス (`ene-cli`)

AI キャラクターとの対話、ツールテスト、メモリ/セッション管理のための対話型 REPL。

## 起動

```bash
cargo run -p ene-cli
# ツールテストモード:
cargo run -p ene-cli -- --tooltest
```

## アーキテクチャ

```
main.rs → clap 引数解析
  → config::init() → 設定読み込み, EneHandle::new()
  → AppContext { handle: EneHandle }
  → repl::run() → dialoguer 入力ループ
      → process_stream() → EneEvent バリアント処理
      → commands::execute() → / プレフィックスコマンド処理
```

CLI は起動時に `EneHandle`（アクター）を作成。ユーザー入力は `handle.run()` で送信し、イベントは `handle.subscribe()` で受信。

## REPL コマンド

コマンドは `/` プレフィックスで入力:

### セッションコマンド

| コマンド | 動作 |
|---------|------|
| `/quit` | REPL を終了 |
| `/clear` | 会話履歴をクリア |
| `/history` | 会話履歴を表示 |
| `/prompt` | 現在のシステムプロンプトを表示 |

### キャラクターコマンド

| コマンド | 動作 |
|---------|------|
| `/card <path>` | 別のキャラクターカードを読み込み (非同期) |

### 設定とツール

| コマンド | 動作 |
|---------|------|
| `/config` | 現在の設定を表示 |
| `/tools` | 有効な全ツールを一覧 |
| `/undo` | 最後のファイル操作を取り消し |
| `/tooltest [prompt]` | ワンショットツールテストを実行 |

### メモリコマンド

| コマンド | 動作 |
|---------|------|
| `/memory search <query>` | 長期記憶を検索 |
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
