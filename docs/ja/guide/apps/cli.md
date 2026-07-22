# CLI リファレンス (`ene-cli`)

AI キャラクターとの対話、ツールのテスト、記憶とセッションの管理を行う対話型 REPL。

## 起動

```bash
cargo run -p ene-cli
```

## コマンドラインフラグ

| フラグ | 動作 |
|------|--------|
| `-h`, `--help` | 使用方法を表示して終了 |
| `-V`, `--version` | バージョンを表示して終了 |
| `--config <PATH>` | デフォルトの場所ではなく指定パスの `settings.json` を読み込む |
| `--character <NAME>` | 設定の既定値ではなく指定のキャラクターカード名またはパスを使用 |
| `--lang <LANG>` | UI 言語を上書き（`en` または `ja`）。既定はシステムロケール |

ユーザー向けの CLI 出力は `apps/ene-cli/i18n/{en-US,ja}/ene_cli.ftl` 配下の Fluent カタログでローカライズされます。有効な言語は `--lang` で上書きしない限りシステムロケールから交渉されます。

## 非対話モード (#186)

サブコマンドなしで `ene` を起動すると対話型 REPL が始まります。サブコマンドを
指定すると、ランタイムに対して単一の操作を実行して終了します。CI パイプラインや
シェルスクリプトでの利用に適しています。グローバルフラグ（`--config`、
`--character`、`--lang`）はどちらのモードでも有効です。

```bash
cargo run -p ene-cli -- run "hello"            # 1 プロンプトを実行しテキストをストリームして終了
cargo run -p ene-cli -- run --jsonl "hello"    # 1 行 1 JSON イベント
cargo run -p ene-cli -- run --json "hello"     # 単一の JSON サマリオブジェクト
cargo run -p ene-cli -- tool list --json
cargo run -p ene-cli -- session list --json
cargo run -p ene-cli -- memory search "tea" --json
cargo run -p ene-cli -- doctor --json
```

### サブコマンド

| サブコマンド | 動作 |
|------------|------|
| `run [PROMPT…]` | 単一プロンプトを実行し応答をストリームして終了。`PROMPT` がない場合は stdin から読み込む |
| `run --jsonl` | 1 行 1 JSON オブジェクト（ストリーミングイベント）を stdout に出力 |
| `run --json` | 単一の JSON サマリオブジェクトを stdout に出力（`--jsonl` と排他） |
| `run --timeout <SECONDS>` | 指定秒数以内にターンが完了しなければ中断 |
| `run --yes` | 副作用を伴うツール操作を自動承認。指定しない場合、権限ゲートはプロンプトを出さず実行を失敗させる |
| `tool list\|search\|help\|call [--json]` | ツールの管理と呼び出し（REPL の `/tool` コマンドに対応） |
| `session list\|export\|import\|search\|archive\|unarchive [--json]` | 会話セッションの管理（`/session` に対応） |
| `memory list\|inspect\|search [--json]` | 認知メモリの検査（`/memory` に対応） |
| `doctor [--json]` | 環境ヘルスチェックの実行（`/doctor` に対応） |
| `store backup\|list-backups\|restore\|integrity [--json]` | メモリ DB のバックアップ / リストア / 整合性チェック (#239) |

### 出力契約

- 構造化結果（`--json` / `--jsonl`）は **stdout** に出力。ログと進捗は **stderr** に留まる。
- `run --jsonl` はチャットイベントバスを映す安定したイベントストリームを出力する:
  `turn_started`、`text_delta`、`performance`、`tool_call_start`、
  `tool_call_result`、`permission_denied`、そして終端の `terminal` イベントが
  ちょうど 1 回（`reason` は `done`、`failed`、`cancelled`、`timeout` のいずれか）。
- 失敗時は JSON エラーエンベロープが stdout に出力される:
  `{"error": {"code": "<class>", "message": "…"}}`。エラークラスは
  `usage`、`runtime`、`timeout`、`tool_failed`、`busy`、`confirmation_required`。

### 終了コード

| コード | 意味 |
|------|------|
| `0` | 成功 |
| `1` | 汎用のランタイムまたは実行失敗 |
| `2` | 無効な引数または使用方法（clap と一致） |
| `3` | 操作が `--timeout` を超過 |
| `4` | ツール呼び出しが失敗 |
| `5` | ランタイムがビジー、またはアクターが利用不可 |
| `6` | 副作用を伴う操作に `--yes` 確認が必要だった |
| `130` | Ctrl-C による中断 |

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
| `/undo` | 直近の可逆なツール操作（filesystem の write/edit/patch/delete）を取り消す。不可逆な操作（shell 実行など）は警告のみで取り消し不可 |
| `/permissions list` | セッション全体のツール権限付与を一覧表示 |
| `/permissions revoke <id>` | id を指定して権限付与を1件取り消す |
| `/permissions reset` | セッション全体の権限付与をすべて取り消す |

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
| `/memory status` | 型付きメモリストアが有効かどうかを表示（pending 書き込み件数を含む） |
| `/memory pending` | 遅延メモリ書き込みの再試行キューを一覧 |
| `/memory retry` | pending を即時 due にしてドレイン |

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
| `/session list` | 保存済みセッションを一覧 (新しい順) |
| `/session export <id>` | セッションをバージョン管理・秘匿化済みの JSON バンドルにエクスポート |
| `/session import <path>` | JSON エクスポートファイルからセッションをインポート |
| `/session search <query>` | 保存済み会話メッセージの全文検索 |
| `/session archive <id>` | セッションをアーカイブ |
| `/session unarchive <id>` | セッションのアーカイブを解除 |

### 診断

| コマンド | 動作 |
|---------|------|
| `/doctor` | 環境ヘルスチェックを実行し、カラー表示のサマリーを出力 |
| `/doctor --json` | 同一チェックを実行し、機械可読な JSON を出力 |
| `/store backup` | メモリ DB のタイムスタンプ付きファイルバックアップを作成 |
| `/store list-backups` | `{db}.bak.*` バックアップを一覧（新しい順） |
| `/store restore <path>` | バックアップから復元（ストアを安全に開き直すため終了する） |
| `/store integrity` | `PRAGMA integrity_check` を実行 |

`/doctor` は以下のカテゴリを検査し、各チェックをステータス（`OK` / `WARN` /
`ERROR` / `SKIP`）と、問題が見つかった場合の修復ヒント付きで報告します:

| カテゴリ | チェック内容 |
|----------|------------|
| Runtime | アクターの応答性（スナップショット往復、セッション、ターン数） |
| Config | キャラクターカードの読み込み状態 |
| AI Provider | チャットプロバイダーの解決と接続性（軽量な models-list 呼び出し、タイムアウト約5秒、ユーザーデータ送信なし）。`ai.fallback.enabled` の場合、設定済みの全クラウドプロバイダーのヘルス（ステータス、レイテンシ、最終エラー）もプローブし、フェイルオーバーポリシー（#175）を報告 |
| Embedding | 埋め込みバックエンドの解決（クラウドまたはローカル） |
| Store | メモリストアの有効化とランタイムでの利用可能性 |
| Tool Registry | ツール登録状況 |
| Assets | assets ディレクトリの存在 |

シークレットは全文出力されません: API キーは短いマスク付きプレフィックス
（例: `sk-…abcd` または `[redacted]`）で表示され、絶対プライベートパスは
`~/…` または末尾コンポーネントに短縮されます。

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
