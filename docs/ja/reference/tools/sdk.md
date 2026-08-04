# ツール SDK リファレンス

> **クレート**: `ene-plugin` | `ene-plugin-proto` | `ene-plugin-db` | `ene-plugin-macros`

これは**ツールプラグイン**（名前空間付きアクションを公開し、プラグイン IPC
経由でホストから駆動される外部プロセスのバイナリ）を作成するためのリファレンスです。
ステップバイステップの作成ガイドは
[ツールの作成](../guide/tools/write-a-tool.md) を参照してください。

この文書の名前が実際のものです。初期の設計文書にあった `ene-tool` /
`ene-tool-derive` / `ene-tool-proto` / `ene-tool-host` / `ene-tool-db` /
`run_tool_server` は `ene-plugin-*` ファミリーへ統合され、旧名は廃止されました。

---

## クレート構成

| クレート | 役割 | 使用箇所 |
|---|---|---|
| `ene-plugin` | オーサリング API: `ToolAction`, `ActionSetProvider`, `SingleActionProvider`, `prelude::tool`, `run_plugin_server`, `PluginDispatch`, `ToolProviderPlugin` | ツールバイナリ |
| `ene-plugin-proto` | ワイヤ ABI: IPC フレーミング、ハンドシェイク、`ToolSpec`, `ToolError`, `SideEffects`, `SandboxConfigData`, `VersionRange`（`ene-plugin` から再エクスポート） | ツールバイナリ + ホスト |
| `ene-plugin-macros` | プロシージャルマクロ: `ToolAction`, `ToolSpec`（`ene-plugin::prelude::tool` から再エクスポート） | ツールバイナリ |
| `ene-plugin-db` | DB IPC クライアント: `DbClient`, `DbSchema`, `DbFilter`, `DbValue`, `batch` | 状態を持つツールバイナリ |
| `ene-plugin-host` | ホスト側のプロセス管理・能力ルーティング・登録（プラグインを消費する側） | コアのみ — ツールの依存にしてはいけない |

## オーサリング API

ツール作成に必要な API は 1 行のインポートで揃います:

```rust
use ene_plugin::prelude::*;
```

これで `ToolAction`、`ToolAction`/`ToolSpec` の derive、`ToolSpec`、
`ToolError`、`schemars::JsonSchema`、`serde::Deserialize`、
`async_trait::async_trait`（および `ActionSetProvider` /
`SingleActionProvider`）が入ります。

### アクションのパターン

アクションは、フィールドが JSON 引数となる構造体です。
`#[derive(ToolAction)]` マクロが以下を生成します:

- 固有の `const TOOL_NAME: &'static str` と `spec() -> ToolSpec`
  （構造体に対する `schemars` の JSON Schema に `#[tool(...)]` の
  メタデータを合成したもの）、
- `name()` / `definition()` / `rag_profile()` をそれらに転送する
  `impl ToolAction`、
- `execute(&self, arguments: &str)` — 引数をデシリアライズし（パース失敗は
  `ToolError::InvalidArguments` に変換）、`#[tool(skip)]` フィールドを
  `self` からコピーして、手書きの
  `async fn run(&self) -> Result<String, ToolError>` を呼び出します。

```rust
#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "calc",
    name = "evaluate",
    summary = "Evaluate a mathematical expression.",
    description = "Evaluates a math expression such as \"2 + 3\".",
    category = "Utility",
    keywords_primary = "calculate, compute, math",
    side_effects = "ReadOnly"
)]
pub struct EvaluateAction {
    /// The expression to evaluate.
    expression: String,
}

impl EvaluateAction {
    async fn run(&self) -> Result<String, ToolError> {
        // validate, compute, return a JSON string
    }
}
```

### `ToolAction` トレイト

```rust
pub trait ToolAction: Send + Sync {
    fn name(&self) -> &'static str;
    fn definition(&self) -> ToolSpec;
    fn rag_profile(&self) -> ToolRagProfile;
    async fn execute(&self, arguments: &str) -> Result<String, ToolError>;
}
```

`ToolAction` は同期のリクエスト–レスポンス型アクションをモデル化しています。
バックグラウンド（遅延）実行はトレイトの対象外です。詳細は
[遅延実行](#遅延実行) を参照してください。

### `ActionSetProvider` とフック

`ActionSetProvider` は `Vec<Box<dyn ToolAction>>` をレガシー `ToolProvider`
インターフェースへ適合させ、ツールバイナリがディスパッチを手書きする必要を
なくします:

```rust
let provider = ActionSetProvider::new(vec![
    Box::new(action::GetAction::new(state.clone())),
    Box::new(action::IncrementAction::new(state.clone())),
])
.with_set_call_context_hook(|conversation_id, turn_id| { /* session state */ })
.with_sandbox_hook(|sandbox| { /* DB socket, auth token */ })
.with_approve_permission_hook(|request_id| { /* record approval */ })
.with_allow_pattern_hook(|action, target_pattern| { /* session allow */ })
.with_revoke_pattern_hook(|action, target_pattern| { /* revoke allow */ })
.with_set_config_hook(|config| { /* plugin config */ })
.with_config_schema_hook(|| Some(schema));
```

各フックは `ToolProvider` トレイトの 1 メソッド
（`set_call_context`, `set_sandbox`, `approve_permission`, `allow_pattern`,
`revoke_pattern`, `set_config`, `config_schema`）に対応します。共有状態や
遅延実行のために `ActionSetProvider` を自前の `ToolProvider` impl でラップする
場合は、使用するライフサイクルメソッドをすべて転送してください。転送しないと
フックは呼ばれません。

### サーバーエントリポイント

```rust
#[tokio::main]
async fn main() {
    let provider = provider::MyToolProvider::new();
    if let Err(e) = run_plugin_server(PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin::new(provider))),
        None, None, None, None,
    )).await {
        tracing::error!("[ene-plugin-my] Fatal error: {e}");
        std::process::exit(1);
    }
}
```

`run_plugin_server` はソケットパスを `ENE_PLUGIN_SOCKET` から読み、ハンドシェイクに
迅速に応答してリクエストをディスパッチします。`PluginDispatch` の 5 スロットは
ツール / LLM / embed / TTS / STT で、ツールバイナリは最初の 1 つだけを使います。

## ツール ABI 互換性テーブル

プラグイン ABI は `PLUGIN_IPC_PROTOCOL_VERSION` でバージョン管理され、
接続ごとにネゴシエーションされます。完全なプロトコル説明は
[プラグインと MCP](../../concepts/plugins-and-mcp.md) を参照してください。
作成者に必要なルールは次のとおりです:

| 項目 | ルール |
|---|---|
| ハンドシェイク | ホストが `VersionRange::host_supported()` を送信し、プラグインは自前のレンジと交差させて合意バージョンを返す |
| 後方互換性 | ホストは N-1 を維持。プラグインはビルド時のバージョンに `VersionRange { min: N, max: N }` で固定してよい |
| フィールド追加 | `#[serde(default)]` を使う。追加的なワイヤ変更はバージョンアップ不要 |
| 削除・リネーム | バージョンアップが必要 — 小さな変更でも行わない |
| 新メッセージ | 古いピアに送れないメッセージを送らないよう `negotiated_version()` やケーパビリティフラグでゲートする |
| `ToolSpec` | `side_effects` と `background_capable` は旧バイナリが省略しても安全な値（`None` / `false`）にデフォルトされる |

## `ToolSpec` のフィールド

`ToolSpec` はモデルが見る情報とホストの実行メタデータで構成されます:

- `name: ToolName` — 検証済みの名前空間付き名前（例: `counter.get`）。
- `description` — LLM に渡す完全な Markdown 説明。
- `parameters` — 構造体から導出される JSON Schema（derive は
  `additionalProperties: false` を強制）。
- `background_capable` — 既定は `false`。`true` で遅延実行にオプトイン。
- `side_effects` — 既定は `None`（"不明"）。以下を参照。

### 副作用と並列ディスパッチ

`SideEffects` は `#[tool(...)]` 属性で宣言します:

```rust
side_effects = "ReadOnly"       // 観測可能な副作用なし
side_effects = "Idempotent"     // 同じ引数 ⇒ 同じ効果
side_effects = "Destructive"    // データ損失の可能性、ロールバック保証なし
side_effects = "FileSystem"     // ファイル操作（mutates フラグ）
side_effects = "Network"        // ネットワークアクセス（external フラグ）
side_effects = "System"         // プロセス起動・シグナル（privileged フラグ）
side_effects = "Browser"        // DOM 自動操作（mutates_dom フラグ）
```

並列ディスパッチは**フェイルクローズ**です。明示的な
`SideEffects::ReadOnly` だけが並列実行の対象になり、`None`（不明）、
`Idempotent`、およびすべての変更系カテゴリは逐次実行のままです。
書き込みを `ReadOnly` と宣言してはいけません（並列化されてしまいます）。
"データベース書き込み" カテゴリは存在しないため、冪等でない状態変更は
属性を省略し、不明デフォルトで逐次実行に留めてください。

## `ToolError` 分類

| バリアント | 用途 |
|---|---|
| `NotFound { tool_name }` | 不明なツール名。`ActionSetProvider` が自動で返す |
| `InvalidName { reason }` | 不正なツール名（ホスト側エントリポイント） |
| `DuplicateName { tool_name }` | 登録時の名前衝突 — 先勝ちはなくハードエラー |
| `InvalidArguments { message }` | 引数のパース失敗または検証失敗 |
| `Generic { kind, message }` | `ErrorKind` で区別するメッセージのみのエラー |
| `PermissionRequired { request_id, action, target, description }` | 機密操作の前にユーザーへ確認 |
| `UserInputRequired { request_id, prompt }` | 選択肢付きの対話的質問 |
| `FileNotFound` / `FileTooLarge` | ファイルシステム操作の失敗 |
| `CommandBlocked` / `ShellTimeout` / `ShellOutputTooLarge` | シェル操作のサンドボックス違反 |

`Generic` にはコンストラクタを使いましょう: `ToolError::execution_failed`、
`::permission_denied`、`::io_error`、`::timeout`、`::internal`、
`::ipc_transport`、`::ipc_client`、`::sandbox_violation`。
`ErrorKind::Other` は非推奨です — ツール側の想定外は `Internal` を使ってください。

`PermissionRequired` は権限フローの構造化プロンプトです。`description` は
ユーザー向けのみで、ホストの監査証跡には記録されません。`target` はプライベート
情報を含まない安定した識別子にしてください。

## 権限フロー

1. アクションは機密操作の**前に** `ToolError::PermissionRequired` を返します。
2. ホストがユーザーに確認し、`approve_permission(request_id)`（1 回限り）か
   `allow_pattern(action, target_pattern)`（セッション中許可）を呼ぶか、
   呼び出しを破棄します。
3. 承認後、ホストは**同一引数でツールを再呼び出し**します。再試行は
   *同じ* `request_id` を生成し、記録済みの承認を認識する必要があります。
   そうしないと毎回同じ確認が繰り返されます。

`ApprovalGate` パターン（`plugins/tool/counter/src/approval.rs` を参照）はこれを
実装しています: `action:target:description` から導出する決定論的リクエスト ID、
`set_call_context` によるターン単位の失効、会話変更時にクリアされるセッション
全体の許可パターンです。

## DB IPC（`ene-plugin-db`）

状態を持つツールはホストサービスの `db` パッセンジャー経由で共有 `memory.db`
にアクセスします。ツールバイナリが独自の SQLite 接続を開くことはありません。

1. **スキーマ宣言**: 起動時に `DbClient::declare_schema` で宣言します。
   テーブル名・インデックス名はプラグインのプレフィックス（例: `counter_`）で
   始まる必要があります。サーバーはプレフィックス分離と識別子検証を強制し、
   DDL は `DeclareSchema` の結果としてのみ実行されます。
2. **CRUD**: `select`、`insert`、`upsert`、`update`、`delete`、`count`。
   フィルタは `DbFilter::eq` など。`Row` は `BTreeMap<String, DbValue>` です。
3. **トランザクション**: `DbClient::batch` は `DbWriteOp` のリストを単一の
   SQLite トランザクションで適用します — 全適用か全ロールバックのどちらかです。
   サーバーは 1 バッチあたり最大 10,000 オペレーションに制限します。
4. **クォータ**: `plugins.list.<name>.db_quota_mb` が共有 DB 内のプラグイン
   フットプリントを制限します（既定 256 MiB）。上限を超える容量増加書き込みは
   `QUOTA_EXCEEDED` で失敗し、読み取り・削除は許可されたままなので空きを
   作れます。
5. **スキーマ進化**: 追加的な変更（新テーブル、新カラム）は自動適用されます。
   競合する変更（型変更、テーブル/カラム削除、制約付き新カラム）は
   `SchemaConflict` で拒否されます。

接続パラメータはサンドボックスハンドシェイクから渡ります:
`SandboxConfigData.db_socket`（パス）と `db_auth_token`（事前共有トークン。
サーバーは未認証接続を拒否）。`set_sandbox` フックで受け取り、最初の使用時に
ストアを遅延構築します。

## 遅延実行

`ToolAction` は同期のリクエスト–レスポンスです。バックグラウンド処理には、
`call_tool_deferred` / `poll_deferred` / `cancel_deferred` を自前のタスク
レジストリでオーバーライドする手書きの `ToolProvider` 実装が必要です。
`ActionSetProvider` は意図的に同期デフォルトのままです。また spec に
`background_capable = true` を設定してください（derive の
`background_capable` 属性）。動作例は utility ツールのタスクレジストリを
参照してください。

## 命名規則

- プラグイン名は `[a-zA-Z0-9_-]` に一致し、バイナリ名は `ene-plugin-<name>`
  で実行可能ビットが必要です（ホストのディスカバリはこのプレフィックスのみを
  走査します）。
- ツール名は `<namespace>.<action>`: ASCII 英数字、`_`、`.`、`:` のみ。
  先頭・末尾の `.`/`:` なし、区切り文字の連続なし。`-` は**不可** —
  名前空間ではハイフンをアンダースコアに変換します。
- 名前空間は通常プラグイン名と同じです（`calc.*`、`geo.*`、`counter.*`）。
  `fs` だけ例外で `filesystem.*` です。
- DB プレフィックスはプラグイン名に従います: `counter` プラグインは `counter_`。

## ログ出力

- **stdout は IPC チャネルです。** stdout への出力はワイヤプロトコルを破壊する
  ため禁止です（`print_stdout` はプラグインクレートでワークスペース全体として
  拒否されています）。
- `tracing` マクロを使い、stderr へ構造化フィールド付きで出力します
  （`tracing::info!(component = "PluginServer", ...)`）。
- 致命的エラーは `tracing::error!("[ene-plugin-<name>] Fatal error: {e}")` の後に
  `std::process::exit(1)` — メッセージなしで死んだプラグインはデバッグ不能です。
- トークン・API キー・ユーザーのプライベート情報はログに残さないでください
  （権限の `description` は意図的にホスト監査証跡から除外されています）。

## 関連

- [ツールの作成ガイド](../guide/tools/write-a-tool.md)
- [プラグインと MCP の概念](../../concepts/plugins-and-mcp.md)
- [ツール開発クレート](../../crates/tool-sdk.md)
- 生成 rustdoc: `cargo doc -p ene-plugin --open`、`cargo doc -p ene-plugin-db --open`
