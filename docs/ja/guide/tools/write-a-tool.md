# ツールの作成

このガイドでは、テンプレートから新しいツールプラグインを作成し、アクションの
実装、権限と DB 状態の配線、テスト、登録までの手順を説明します。API サーフェスと
ワイヤ ABI のリファレンスは [ツール SDK リファレンス](../reference/tools/sdk.md)
を参照してください。完全な動作サンプルとして `plugins/tool/counter`
（DB バックアップのカウンターと権限ゲート付きリセット）が同梱されているので、
このガイドとあわせて読んでください。

## 1. テンプレートから雛形を作る

```sh
templates/tool/new-tool.sh my_tool
```

これで `plugins/tool/my_tool/` が生成され、クレート名・バイナリ名は
`ene-plugin-my_tool`、名前空間は `my_tool`、アクションは `my_tool.echo` の
1 つになります。ワークスペースの `Cargo.toml` の `plugins/tool/*` グロブが
新しいディレクトリを自動的に拾うため、マニフェストの編集は不要です。

テンプレートの構成:

```text
plugins/tool/my_tool/
├── Cargo.toml          # workspace 依存、[[bin]] ene-plugin-my_tool
└── src/
    ├── main.rs         # run_plugin_server エントリ、致命的エラーパス
    ├── action.rs       # derive ベースのアクション 1 つ（検証 + テスト付き）
    └── provider.rs     # ActionSetProvider のラッパー
```

## 2. アクションを実装する

アクションは、フィールドが JSON 引数となる構造体です。`ToolAction` を derive
（`ToolSpec` を含む）し、`run` を書きます:

```rust
use ene_plugin::prelude::*;

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "my_tool",
    name = "greet",
    summary = "Greet a person.",
    description = "Returns a greeting for the given name.",
    category = "Utility",
    keywords_primary = "greet, hello",
    side_effects = "ReadOnly"
)]
pub struct GreetAction {
    /// The name to greet.
    #[arg(min_length = 1, max_length = 100)]
    name: String,
}

impl GreetAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(serde_json::json!({ "greeting": format!("Hello, {}!", self.name) }).to_string())
    }
}
```

derive は spec、`name()`/`definition()` のフォワーダ、`execute()`
（デシリアライズ → `#[tool(skip)]` フィールドをコピー → `run` を呼ぶ）を
生成します。ツール名は `my_tool.greet` になり、ディスパッチ名と spec 名は
構造的に同一文字列になります。

### スキーマとバリデーション

- フィールドの doc コメントは JSON Schema の説明になり、`#[arg(...)]` で
  制約を追加できます（`minimum`/`maximum`、`min_length`/`max_length`、
  `min_items`/`max_items`、`enum_values`、`default`、`description`、
  `hidden`、`internal`、`skip`）。
- **スキーマ制約は実行時チェックではありません。** 生成される `execute()` は
  JSON パース失敗のみをエラーにします。ビジネスルールは `run` 内で検証し、
  `ToolError::InvalidArguments { message }` を返します:

```rust
if self.name.trim().is_empty() {
    return Err(ToolError::InvalidArguments {
        message: "name must not be empty".to_string(),
    });
}
```

- パース後の文字列加工より型付きフィールドを優先します（自由文字列 +
  `enum_values` より enum 型）。
- 引数でない状態（共有ストア、セッション、設定）は `#[tool(skip)]` フィールド
  + `#[serde(skip, default = "...")]` に置きます — derive が `execute()` で
  `self` から再コピーします。

## 3. プロバイダを配線する

アクションを `ActionSetProvider` に登録し、ライフサイクル状態をフック経由で
渡します:

```rust
let inner = ActionSetProvider::new(vec![
    Box::new(action::GreetAction::default()),
    Box::new(action::IncrementAction::new(state.clone())),
])
.with_set_call_context_hook(|conversation_id, turn_id| {
    state.set_session_id(conversation_id);
    state.gate().on_call_context(conversation_id, turn_id);
})
.with_sandbox_hook(|sandbox| {
    if let Some(socket) = &sandbox.db_socket {
        state.set_db_socket(socket.clone());
    }
    state.set_db_auth_token(sandbox.db_auth_token.clone());
});
```

共有状態や遅延実行のために `ActionSetProvider` を自前の `ToolProvider` impl で
ラップする場合は、使うライフサイクルメソッド（`set_sandbox`、
`set_call_context`、`approve_permission`、`allow_pattern`、`revoke_pattern`、
`set_config`、`config_schema`）をすべて転送してください。転送しないとフックは
静かに発火しません。

## 4. 副作用と権限を宣言する

### 副作用

`#[tool(...)]` の `side_effects` は実行メタデータです: 並列ディスパッチが
許されるのは `"ReadOnly"` だけです。それ以外（属性の省略 = "不明" を含む）は
逐次実行です。正直に宣言しましょう:

| アクション | 宣言 | 理由 |
|---|---|---|
| 読み取り専用の参照 | `"ReadOnly"` | 観測可能な副作用なし。並列化可能 |
| 冪等でない状態書き込み | *（なし）* | 不明 → フェイルクローズで逐次。 "DB 書き込み" カテゴリは存在せず、`ReadOnly` と偽ると書き込みが並列化される |
| 本当に冪等な書き込み | `"Idempotent"` | 同じ引数 ⇒ 同じ効果 |
| データ損失の可能性 | `"Destructive"` | 慎重な扱いを促す。下の権限フローと併用する |

### 権限と破壊的アクション

破壊的・プライバシーに関わるアクションはユーザーに確認が必要です。流れ:

1. `run` は操作を実行する**前**に承認ゲートを呼びます。
2. 未承認ならゲートは `ToolError::PermissionRequired { request_id, action,
   target, description }` を返します。
3. ホストが確認し、承認されると `approve_permission(request_id)`
   （またはセッション中許可の `allow_pattern(action, target)`）を呼び、
   **同一引数でツールを再呼び出し**します。
4. 再試行は*同じ* `request_id` を計算して記録済みの承認を見つける必要が
   あるため、ID はランダム UUID ではなく `action:target:description` の
   決定論的ハッシュにします。

```rust
self.state.gate().check(
    "MyToolDelete",                     // 正規のアクション名
    "my_tool:delete",                   // 安定したターゲット ID（秘密情報なし）
    "Delete the selected item",         // ユーザー向けプレビュー
)?;
```

`plugins/tool/counter/src/approval.rs` の `ApprovalGate` が完全なパターンを
実装しています: `set_call_context` によるターン単位の失効、セッション全体の
許可パターン、失効処理です。`target` は秘密情報を含まない安定した識別子に —
`description` はユーザー向けのみで、ホストの監査証跡には入りません。

## 5. タイムアウト

外部呼び出しには必ず上限を設けます:

- HTTP: クライアントにタイムアウトを設定（`reqwest::Client::builder().timeout(...)`）
  — geo ツールは 10 秒。レスポンス本文サイズも上限をかけます。
- アクション内の待機: `tokio::time::timeout(Duration, future)` を使い、
  経過時は `ToolError::timeout("...")` に変換します。
- DB/IO 失敗: `ToolError::internal(...)`（I/O なら `::io_error`）に変換 —
  生の `DbError` 文字列をモデル向けエラーにそのまま流さないでください。

## 6. キャンセルと遅延実行

`ToolAction` は同期のリクエスト–レスポンスで、ホストがキャンセルできるのは
実行中の同期呼び出しではなく遅延タスクです。バックグラウンド処理には:

1. アクションに `background_capable = true` を宣言します。
2. `ActionSetProvider` をやめて手書きの `ToolProvider` にし、
   `call_tool_deferred`（仕事を開始して `DeferredOutcome::Deferred { task_id }`
   を返す）、`poll_deferred`（`Pending`/`Completed`/`Cancelled`/`Unknown` を
   返す）、`cancel_deferred`（タスクのキャンセルハンドルに通知）を
   オーバーライドします。

utility ツールの `TaskRegistry` が参考実装です。キャンセルは協調的です:
変更途中で中断するのではなく、自然な中断ポイントでキャンセルフラグを確認します。

## 7. 状態を持つツール: DB IPC

独自の SQLite 接続を開かないでください — プラグインはホストサービスの `db`
パッセンジャー経由で共有 `memory.db` にアクセスします（`ene-plugin-db`）。

1. ストア構築時にスキーマを一度宣言します。テーブル名・インデックス名には
   プラグインのプレフィックス（`my_tool_`）が必要です:

```rust
DbSchema {
    prefix: "my_tool_".to_string(),
    tables: vec![DbTable { /* ... */ }],
    indexes: vec![],
}
```

2. サンドボックスハンドシェイクのデータ（`db_socket` + `db_auth_token`）から
   counter ツールの `ensure_store()` のように遅延接続します。
3. `DbClient` は `Arc<tokio::sync::Mutex<>>` で包みます — 全操作が
   `&mut self` を取り、ストアはアクションと並行呼び出しで共有されます。
4. まとめて適用する必要のある複数行書き込みは `DbClient::batch` で
   （単一トランザクション、上限 10,000 オペレーション）。
5. プラグインごとのクォータ（`plugins.list.<name>.db_quota_mb`、既定
   256 MiB）を尊重します。`QUOTA_EXCEEDED` は内部エラーとして扱い、
   削除はゲートされないので満杯のプラグインは空きを作れます。

## 8. テストとモック

### 単体テスト（バイナリクレート内）

アクション/プロバイダモジュール内の `#[cfg(test)] mod tests` で、
`run` には `#[tokio::test]` を使います。テンプレートの
`#![cfg_attr(test, expect(clippy::unwrap_used, reason = "..."))]` がテストの
アサーションをカバーします。

### モックのレシピ

- **DB**: ストアをトレイト（counter サンプルの `CounterStore`）で抽象化し、
  インメモリ実装（`InMemoryCounterStore`）を用意します。`#[cfg(test)]` のシーム
  （`CounterState::set_test_store`）で注入すれば、DB サーバーなしでアクションを
  実行できます。
- **権限拒否**: 新しいゲートに対してアクションを実行し
  `ToolError::PermissionRequired` を検証。`request_id` を取り出して
  `gate.approve_request(&request_id)` を呼び、再実行して成功を検証します。
- **不正リクエスト**: `execute` に不正な JSON を渡して `InvalidArguments` を
  検証し、意味的に不正な値でも `run` の `InvalidArguments` メッセージを
  検証します。
- **Not found / ディスパッチ**: 未知の名前でプロバイダを呼び、
  `ToolError::NotFound` を検証します。

### IPC 統合テスト

`plugins/tool/counter/tests/ipc.rs` がレシピです: 実際のバイナリを
（`env!("CARGO_BIN_EXE_ene-plugin-counter")`）一意な `ENE_PLUGIN_SOCKET` 付きで
起動し、`IpcStream` で接続、ハンドシェイクを行い、`ListTools` / `CallTool` /
`ApprovePermission` をワイヤ上で駆動します。バイナリ専用クレートで動作するため
`[lib]` ターゲットは不要です。

これらのテストは意図的に DB サーバーなしで実行します。アクションはストアに
触れる**前**に引数検証と権限チェックを行うため、権限拒否・不正リクエストの
ケースをエンドツーエンドで検証でき、承認後の再試行はストア境界で
`ErrorKind::Internal` になり、サンドボックス→ストアの配線を証明します。

## 9. 登録と確認

1. バイナリをビルドします（`cargo build -p ene-plugin-my_tool`）。ホストは
   実行可能名 `ene-plugin-<name>` でプラグインを発見します。
2. プラグインを有効化します: `settings.json` の `plugins.list` に
   `"my_tool": { "enable": true }` を追加するか、組み込みにする場合は
   `crates/ene-plugin-host/src/config.rs` の `default_plugin_list()` に
   追加します。
3. アプリを起動して `/tool list` に新アクションが表示されることを確認します。
   `/tool call my_tool.greet '{"name":"Ene"}'` で往復を確認できます。

## 10. 互換性・命名・ログ

- プラグインはビルド時のプロトコルバージョンに固定し、ホスト側の N-1
  ネゴシエーションに任せます。フィールド追加は `#[serde(default)]` で行い、
  ワイヤバリアントの削除・リネームはしないでください。
- プラグイン名: `[a-zA-Z0-9_-]`。バイナリ: `ene-plugin-<name>`。名前空間:
  `[a-zA-Z0-9_.:]`（ハイフンはアンダースコアに）。DB プレフィックス: `<name>_`。
- stdout は IPC チャネルです — `println!` は禁止、`tracing` のみ。
  致命的エラーは `tracing::error!` + `std::process::exit(1)` で記録し、
  トークンやユーザーのプライベート情報はログに残さないでください。

## 関連

- [ツール SDK リファレンス](../reference/tools/sdk.md)
- [プラグインと MCP の概念](../../concepts/plugins-and-mcp.md)
- ツール別ガイド: [ランダム](../guide/tools/random.md)、[地理情報](../guide/tools/geo.md)、
  [Git](../guide/tools/git.md)
