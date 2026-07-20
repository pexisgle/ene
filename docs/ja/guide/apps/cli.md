# CLI リファレンス (`ene-cli`)

AI キャラクターとの対話、ツールのテスト、記憶とセッションの管理を行う対話型 REPL。

## 起動

```bash
cargo run -p ene-cli
```

## アーキテクチャ

```
main.rs → clap 引数解析
  → config::init() → ConfigStore::try_load, EneHandle::open(config, card)
  → AppContext { handle: EneHandle, commands: Vec<Arc<dyn CliCommand>> }
  → repl::run() → TerminalUi 行エディタループ
      → stream::process_stream() → EneEvent バリアント処理（TurnId 範囲）
      → commands::execute() → CliCommand トレイト経由の / コマンドディスパッチ
```

ログはツリー対応の `tracing` Layer（`TreeLogLayer`）と `TerminalUi` が連携し、post-turn 行が `>: ` プロンプトを上書きしないようにする。
CLI は起動時に準備済み `EneHandle`（`open`）を作成。ユーザー入力は `handle.run()` → `TurnId` で送信し、イベントは `handle.subscribe()` で受信。

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
| `/clear` | 次回実行時に会話履歴を更新することを示す（このリリースでは手動クリアは no-op） |
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
| `/tool search <query>` | RAGまたは名前/説明フィルタを使用して登録済みツールを検索 |
| `/tool help <name>` | ツールの詳細ヘルプを表示 |
| `/tool call <name> <json>` | JSON 引数でツールを直接呼び出し |
| `/undo` | プレースホルダー (アクターベースランタイムでは未対応) |

### メモリコマンド

| コマンド | 動作 |
|---------|------|
| `/memory list [--kind <kind>]` | 型付き記憶（typed memory）を一覧表示（kind フィルタ可） |
| `/memory inspect <id>` | 型付き記憶（typed memory）の詳細を表示 |
| `/memory search <query>` | 型付き記憶（typed memory）をハイブリッド検索（スコア内訳付き） |
| `/memory why <id>` | メモリの想起/ライフサイクル理由を表示 |
| `/memory pin <id>` | メモリをピン留めし自然減衰対象から除外 |
| `/memory archive <id>` | メモリを archived に遷移 |
| `/memory forget <id>` | メモリを user_deleted に遷移 |
| `/memory dispute <id>` | メモリを disputed に遷移 |
| `/memory restore <id>` | メモリを active に戻す |
| `/memory status` | 型付きメモリストアが有効かどうかを表示 |

### 感情コマンド

| コマンド | 動作 |
|---------|------|
| `/affect show` | 現在の感情状態（Affect）を表示 |
| `/affect reset` | 感情状態（Affect）をニュートラルにリセット |

### コミットメントコマンド

| コマンド | 動作 |
|---------|------|
| `/commitments list` | active な commitments を一覧表示 |
| `/commitments done <id>` | commitment を完了済みにする |

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

進捗・ツールログはカスタム `tracing` Layer が出力する。ネストした span（並列 pre-turn / post-turn）は ASCII ツリー、span 外のイベントは 1 行で表示する。LLM テキストは stdout にストリームする。

各ログ行はレベル色（`INFO` 緑 / `WARN` 黄 / `ERROR` 赤）と出所ラベル（`component` があればそれ、なければ短い tracing target）付きで、例: `INFO MemoryWriter: …`。ツリー上の span 名はシアン。

| チャネル | 内容 |
|---------|------|
| stderr（ツリー / フラット） | パイプライン段階、ツール、post-turn メモリ / affect |
| stdout | `TextDelta` / `[Performance: …]` |

post-turn は `Terminal` のあとも継続する。REPL はすぐに `>: ` を出し、後から届いたログ行は入力中バッファを保ったままプロンプトの**上**に差し込む。

例:

```text
>: hello
|- pre_turn.phase_a
| |- embedding
| | └ Generating user query embedding...
| └ ccv3_sync
|   └ Character card memories already up-to-date
assistant reply text
|- post_turn.memory
| └ Post-turn memory extraction and forgetting completed
>: 
```
