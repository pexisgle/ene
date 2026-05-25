# CLI リファレンス (`ene-cli`)

AI キャラクターとの対話、ツールテスト、メモリ/セッション管理のための対話型 REPL。

## 起動

```bash
cargo run -p ene-cli
# ツールテストモード:
cargo run -p ene-cli -- --tooltest
# API キー設定 (OS キーリングに保存):
cargo run -p ene-cli -- --set-api-key
```

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
| `/card <path>` | 別のキャラクターカードを読み込み |

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
| `/session split` | 手動でセッション分割を実行 |
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

## アーキテクチャ

```
main.rs → clap 引数解析
  → config::init() → 設定読み込み, AiRuntime::init()
  → repl::run() → dialoguer 入力ループ
      → process_stream() → AiStreamEvent バリアント処理
      → commands::execute() → / プレフィックスコマンド処理
```
