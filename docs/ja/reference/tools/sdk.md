# ツール SDK

このページは `ene-plugin` でプラグインを書くためのリファレンスです。
手順書は[ツールを書く](../../guides/tools/write-a-tool.md)を参照してください。

## 1 行インポート

```rust
use ene_plugin::prelude::*;        // 下記すべて + ene_infer の再エクスポート
use ene_plugin::prelude::tool;     // ツール作成のみ
use ene_plugin::prelude::provider; // プロバイダー作成のみ
```

## ツールアクション

アクションは `#[derive(ToolAction)]` した構造体で、フィールドが JSON 引数、
`run(&self) -> Result<..., ToolError>` が挙動です:

```rust
#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "utility",
    name = "get_current_time",
    summary = "Get the current local time.",
    description = "...",
    category = "Utility",
    keywords_primary = "time, clock, now",
    side_effects = "...",      // 任意。下記参照
    background_capable         // 任意: 遅延タスクとして実行
)]
struct GetTimeAction { /* フィールド */ }
```

### `#[tool(...)]` 属性

| 属性 | 意味 |
|---|---|
| `namespace` | ツール名前空間（`<namespace>.<name>` の接頭辞） |
| `name` | アクション名 |
| `summary` | モデルに示す 1 行要約 |
| `description` | ツール選択用の完全な説明 |
| `category` | 表示グループ |
| `keywords_primary` / `keywords_secondary` | 検索キーワード（ツール RAG） |
| `side_effects` | `"FileSystem { mutates: true }"` のような宣言 — 承認をゲート |
| `background_capable` | 遅延バックグラウンドタスクとして実行可能 |

### `#[arg(...)]` フィールド属性

引数フィールドのスキーマ制約: `internal`（非表示）・`enum_values`・
`default`・`minimum`/`maximum`・`min_length`/`max_length`・
`min_items`/`max_items`・`description`。

### `ToolError`

種別とメッセージを持つ構造化・IPC 直列化可能エラー:
`ToolError::internal(...)`・プロバイダーエラー・検証エラーなど。

## プロバイダー

`ActionSetProvider` がアクション一覧を返します:

```rust
impl ActionSetProvider for MyProvider {
    fn actions(&self) -> Vec<Box<dyn ToolAction>> { vec![...] }
}
```

`SingleActionProvider` は 1 アクションを包みます。サーバーエントリポイント:

```rust
run_plugin_server(PluginDispatch::new(
    Some(Arc::new(MyToolProvider)),  // tool
    Some(Arc::new(MyLlm)),           // llm（任意）
    Some(Arc::new(MyEmbed)),         // embedding（任意）
    Some(Arc::new(MyTts)),           // tts（任意）
    Some(Arc::new(MyStt)),           // stt（任意）
)).await
```

`PluginDispatch::new` は 5 つの位置引数（tool・llm・embed・tts・stt）を
この順で取ります。VAD は後から追加されたためビルダーステップ
`.with_vad(plugin)` で、capability 仲介は
`.with_capability_provider(plugin)` / `.with_capability_declarations(...)`
で接続します。

## プロバイダープラグイン

プロバイダープラグインは `LlmPlugin`・`EmbedPlugin`・`TtsPlugin`・
`SttPlugin`・`VadPlugin` と `ConfigurablePlugin`（設定スキーマ）の一部または
全部を実装します。`#[provider(...)]` 属性が仕様を宣言します:

```rust
#[derive(LlmPlugin)]
#[provider(
    kind = "openai",
    models = "gpt-5.6-luna",
    streaming,
    vision,
    context_window = 128000,
    max_in_flight = 8,
    queue_depth = 32,
    resource_class = "cloud",
    provides = "llm/chat@1, embed@1",
    requires = "gguf-runner@1"
)]
struct MyLlm;
```

derive は静的仕様コンストラクタと kind 定数を生成します。非同期ハンドラ
（`chat_stream`・`chat_completion`・`embed_batch` など）と
`*_capabilities()` メソッドは自分で書きます。capability 文字列はコンパイル時
に検証されます。

## ローカル推論の規律

プラグインが自プロセスでモデルを実行する場合（llama.cpp・whisper.cpp・
ONNX）、prelude 経由で再エクスポートされる `ene-infer` フレームワークを
使ってください:

- `LocalModel` を実装（プレーンな同期 `&mut self` トレイト）。
- `EngineHandle::spawn(factory, config)` が専用ワーカースレッドでモデルを
  所有します。
- `EngineHandle::submit(req, token)` で有界キュー・協調キャンセル・単一
  タイムアウト・パニック回復を得られます。共有モデルの周りで
  `spawn_blocking`/`block_in_place` を手書きしないでください。

## 遅延（バックグラウンド）タスク

`background_capable` アクションは遅延モードで呼び出されます。ホストは即座に
タスク ID を返し、プラグインはバックグラウンドで作業し、完了は
`DeferredStatus` として届きます（参照実装は `utility.notify_send`）。
ライフサイクルイベント（`tool_background_completed`）が完了を UI に通知します。

## 状態保持ツールの DB アクセス

永続状態には `ene-plugin-db` を使います:

```rust
let client = ene_plugin_db::client::connect().await?;   // ホスト `db` パッセンジャー
client.insert(&table, &row).await?;
```

テーブルはプラグインごとにプレフィックス分離され、トークン認証されます。
`counter` プラグインが参照サンプルです。

## テスト

- ユニットテストは bin クレート内で実行します（`#[cfg(test)]` モジュール）。
  プラグインクレートは慣例でバイナリのみです。
- ホスト側には契約テスト（`ene-plugin-host` の
  `tests/ipc_integration.rs` パターン）があります。状態保持プラグインは
  `plugins/tool/counter/tests/ipc.rs` のような IPC 統合テストを同梱すべきです。
- `ene-infer` の `conformance` テスト一式（フィーチャー `test-util`）は、
  `LocalModel` 実装のキュー/キャンセル/パニック回復の不変条件を検証します。

## 参考実装

- 最も単純なツール: `plugins/tool/random`
- 遅延/バックグラウンド: `plugins/tool/utility`（`notify_send`）
- 状態保持 + 権限ゲート + IPC テスト: `plugins/tool/counter`
- クラウドプロバイダー: `plugins/provider/openai`
- ローカルモデルプロバイダー: `plugins/provider/local-llm`
- テンプレート: `templates/tool/`（`new-tool.sh`）
