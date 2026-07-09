# `ene-provider` — APIリファレンス

> **クレート:** `ene-provider`
> **役割:** LLM および埋め込みプロバイダーのトレイト定義と組み込み実装。

---

## 概要

`ene-provider` は、Ene ランタイムを特定の AI サービスベンダーから切り離すプロバイダー抽象化レイヤーを定義します。すべての LLM 呼び出しと埋め込み操作は、2つのコア `async` トレイト — `LlmProvider` と `EmbeddingProvider` — を通じて行われます。両方とも失敗を `String` ではなく型付きエラー（`LlmProviderError`、`EmbeddingError`）で報告するため、呼び出し元はバリアントに応じて分岐できます（例：`LlmProviderError::RateLimit` で「レート制限」通知を表示する）。

プロバイダーは起動時に `LlmProviderRegistry` を介して登録され、アプリケーションコードを変更せずに設定（`ProviderConfig`）で切り替えることができます。

```mermaid
flowchart LR
    Core[ene-core] -->|dyn LlmProvider| Registry[LlmProviderRegistry]
    Core -->|dyn EmbeddingProvider| EP[EmbeddingProvider]
    Registry --> OAI[OpenAiProvider]
    EP --> Cloud[CloudEmbeddingProvider]
    EP --> Local[GgufEmbeddingProvider\n(ene-embedding)]
    EP --> Hybrid[HybridRerankProvider]
```

---

## `LlmProvider` トレイト

言語モデルバックエンドのコアインターフェースです。すべてのメソッドが `async` です。

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        tools: &[ene_tool_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    >;

    async fn chat_completion(
        &self,
        messages: &[LlmMessage],
        json_schema: Option<serde_json::Value>,
    ) -> Result<String, LlmProviderError>;
}
```

### メソッドテーブル

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `name` | `fn name(&self) -> &str` | 人間が読めるプロバイダー名（例：`"openai-compatible"`）。`async` ではない。 |
| `create_chat_stream` | `async fn create_chat_stream(&self, messages: &[LlmMessage], tools: &[ToolSpec]) -> Result<Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>, LlmProviderError>` | ストリーミングチャット補完を開く。ユーザーがストリーミング出力を見るすべての対話ターンで使用される。 |
| `chat_completion` | `async fn chat_completion(&self, messages: &[LlmMessage], json_schema: Option<serde_json::Value>) -> Result<String, LlmProviderError>` | 非ストリーミングの補完。JSON スキーマによる制約を任意で指定可能。構造化出力を必要とする内部タスク（例：セッションの要約、リランクのスコアリング）で使用される。 |

---

## `EmbeddingProvider` トレイト

テキスト埋め込みおよびセマンティックユーティリティ操作のためのインターフェースです。デフォルト実装があるか単純なゲッターである `dimensions`/`model_name`/`has_reranker` を除き、すべてのメソッドが `async` です。

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str, kind: EmbeddingKind) -> Result<Vec<f32>, EmbeddingError>;

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        self.embed(text, EmbeddingKind::Query).await
    }

    async fn embed_batch(
        &self,
        items: &[(&str, EmbeddingKind)],
    ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        /* デフォルト: embed() を1アイテムごとに逐次呼び出す */
        unimplemented!()
    }

    async fn hyde(&self, query: &str) -> Result<String, EmbeddingError> {
        Ok(query.to_string())
    }

    fn has_reranker(&self) -> bool {
        false
    }

    async fn rerank(
        &self,
        query: &str,
        candidates: &[ene_tool_proto::ToolSpec],
    ) -> Result<Vec<f32>, EmbeddingError> {
        /* デフォルト: embed_query(query) と embed_batch(candidates) 間のコサイン類似度 */
        unimplemented!()
    }

    fn dimensions(&self) -> usize;
    fn model_name(&self) -> &str;
}
```

### メソッドテーブル

| メソッド | 必須か | デフォルトの動作 |
|---|---|---|
| `embed(text, kind)` | **必須** | — |
| `embed_query(text)` | 任意 | `embed(text, EmbeddingKind::Query)` に委譲する。プロバイダーが別のクエリプレフィックス経路を必要とする場合のみオーバーライドする。 |
| `embed_batch(items)` | 任意 | `embed` を1アイテムごとに逐次呼び出すループ。実際のバッチ処理／並列性のためにオーバーライドする。 |
| `hyde(query)` | 任意 | `query` をそのままエコーバックする（no-op HyDE）。実際の仮想文書を生成するには（通常 LLM 経由で）オーバーライドする。 |
| `has_reranker()` | 任意 | `false`。ネイティブなリランカーがあることを示すためにオーバーライドし、呼び出し元が余計なレイテンシを追加するだけの手動 `rerank()` 呼び出しをスキップできるようにする。 |
| `rerank(query, candidates)` | 任意 | `embed_query` で `query` を埋め込み、各候補の `"{summary} {description}"` を `EmbeddingKind::Description` で `embed_batch` を用いて埋め込み、[`cosine_similarity`](#cosine_similarity) でスコアリングする。返されるスコアは `candidates` と同じ長さで対応する。 |
| `dimensions()` | **必須** | — 出力ベクトルの次元数。 |
| `model_name()` | **必須** | — 人間が読めるモデル識別子文字列。 |

### `EmbeddingKind`

テキストがどのように使用されるかをプロバイダーに伝えるヒントです。プロバイダーは kind ごとに異なるプレフィックスやチャンク化戦略を適用することがあります（例：組み込みプロバイダーは `Query`/`Hyde` テキストに `"Query: "` プレフィックスを付ける）。

```rust
pub enum EmbeddingKind {
    Summary,
    Description,
    Capability,
    Example,
    Negative,
    Query,
    Hyde,
}
```

| バリアント | 意味 |
|---|---|
| `Summary` | 簡潔なツールまたはドキュメントのサマリーテキスト。 |
| `Description` | 完全な説明テキスト（しばしば長い）。 |
| `Capability` | メガツール内の機能／アクションごとに1つの埋め込み。 |
| `Example` | 動作例ごとに1つの埋め込み。 |
| `Negative` | ネガティブキーワードテキスト（Tool RAG での緩やかなペナルティ用）。 |
| `Query` | ユーザークエリ — ドキュメントとは異なるプレフィックスを使うことがある。 |
| `Hyde` | HyDE によって生成された仮想文書。 |

---

## `cosine_similarity`

```rust
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32
```

2つのベクトル間のコサイン類似度です。いずれかの入力が空、または2つの長さが異なる場合は（パニックせずに）`0.0` を返します。意味のある結果を得るには両ベクトルが同じ長さであるべきです。デフォルトの `rerank()` 実装と、[`HybridRerankProvider`](#hybridrerankprovider) のフォールバックパスで内部的に使用されます。

---

## メッセージ型

### `LlmMessage`

任意の LLM プロバイダーに送信される統一メッセージ形式です。`#[serde(rename_all = "snake_case", tag = "role")]` でシリアライズされます。

```rust
pub enum LlmMessage {
    System { content: String },
    User { parts: Vec<UserMessagePart> },
    Assistant {
        content: Option<String>,
        tool_calls: Option<Vec<LlmToolCall>>,
    },
    Tool { tool_call_id: String, content: String },
}
```

| バリアント | 説明 |
|---|---|
| `System { content }` | モデルへの指示。通常はキャラクターカード + 注入されたコンテキスト。 |
| `User { parts }` | テキストとインライン画像を含みうるユーザーメッセージ（`UserMessagePart` を参照）。 |
| `Assistant { content, tool_calls }` | ツール呼び出しの記録を任意で含む、以前のアシスタント応答。 |
| `Tool { tool_call_id, content }` | `tool_call_id` によってキー付けされた、ツール呼び出しの結果。 |

### `UserMessagePart`

```rust
pub enum UserMessagePart {
    Text { text: String },
    Image { base64_image_data: String },
}
```

### `LlmResponseChunk`

LLM からの単一のストリーミングフラグメントです。

```rust
pub struct LlmResponseChunk {
    pub text_delta: Option<String>,
    pub tool_calls_delta: Option<Vec<LlmToolCallChunk>>,
}
```

### `LlmToolCall`

完全に組み立てられたツール呼び出し（履歴／非ストリーミングレスポンスから）です。

```rust
pub struct LlmToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}
```

### `LlmToolCallChunk`

ツール呼び出しのストリーミングフラグメントです。同じ `index` を持つ複数のチャンクを連結して、完全な `LlmToolCall` を構成する必要があります。

```rust
pub struct LlmToolCallChunk {
    pub index: usize,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: Option<String>,
}
```

### `Role`

```rust
pub enum Role {
    System,
    User,
    Assistant,
}
```

会話*履歴*の保存におけるメッセージ作成者のロールを表します（プロバイダーに送信されるワイヤーフォーマットである `LlmMessage` とは異なります）。

---

## `LlmProviderFactory` と `LlmProviderRegistry`

```rust
pub trait LlmProviderFactory: Send + Sync {
    fn provider_name(&self) -> &str;

    fn create_provider(
        &self,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn LlmProvider>, LlmProviderError>;
}
```

ファクトリは、生の `serde_json::Value` ではなく**ライブの `EneConfig`** から具体的な `LlmProvider` を構築します — ファクトリ自身が必要な設定セクションを抜き出す責任を持ちます（通常は `config.get_section::<ProviderConfig>()` を介する）。

```rust
pub struct LlmProviderRegistry { /* 非公開 */ }

impl LlmProviderRegistry {
    pub fn register(factory: Arc<dyn LlmProviderFactory>);

    pub fn create_provider(
        name: &str,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn LlmProvider>, LlmProviderError>;
}
```

プロバイダー名から登録済みファクトリへマッピングする、グローバルな `OnceLock` ベースのシングルトンです。

| メソッド | 説明 |
|---|---|
| `register(factory)` | `factory.provider_name()` の下にファクトリを登録する。値そのものではなく `Arc<dyn LlmProviderFactory>` を受け取る — ファクトリは呼び出しごとに所有されるのではなく共有される。 |
| `create_provider(name, config)` | `name` の下に登録されたファクトリを検索し、`factory.create_provider(config)` を呼び出す。`name` の下にファクトリが登録されていない場合は `LlmProviderError::Provider` を返す。 |

---

## 設定型

`ene_config::define_config!` を通じて定義されます（[`ene-config`](./ene-config.md) を参照）。このクレートから再エクスポートされます。

### `ProviderConfig`

```rust
pub struct ProviderConfig {
    pub name: String,             // デフォルト: "openai-compatible"
    pub model: String,            // デフォルト: "gpt-4o-mini"
    pub base_url: String,         // デフォルト: ""
    pub api_key: ApiKeyConfig,
    pub embedding: EmbeddingConfig,
}
```

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `resolve_base_url` | `fn resolve_base_url(&self) -> Result<String, ene_config::ConfigError>` | `base_url` が空でなければそれを返す。それ以外は `Err(ConfigError::MissingBaseUrl { .. })`。 |
| `resolve_api_key` | `fn resolve_api_key(&self) -> String` | `api_key.source` に応じて API キーを解決する：`"inline"` は `api_key.inline` を使用する（デバッグビルドのみ、`API_TOKEN` 環境変数へフォールバックする）。`"env"` は `api_key.env` で指定された環境変数を読み取る（未設定／空の場合は `"OPENAI_API_KEY"` をデフォルトとする）。その他の値は `"inline"` と同様に動作する。何も解決できない場合はパニックせずに `""` を返す。 |

### `ApiKeyConfig`

```rust
pub struct ApiKeyConfig {
    pub source: String,  // デフォルト: "inline"
    pub inline: String,  // デフォルト: ""
    pub env: String,      // デフォルト: "OPENAI_API_KEY"
}
```

### `EmbeddingConfig`

```rust
pub struct EmbeddingConfig {
    pub backend: String,               // デフォルト: "cloud"
    pub query_prefix: Option<String>,  // デフォルト: None
    pub cloud: CloudEmbeddingConfig,
    pub local: LocalEmbeddingConfig,
}
```

`backend` は `"cloud"`（同じ LLM プロバイダーの埋め込みAPIを使用）と `"local"`（[`ene-embedding`](./ene-embedding.md) を介したローカル GGUF モデルを使用）を切り替えます。

### `CloudEmbeddingConfig` / `LocalEmbeddingConfig`

```rust
pub struct CloudEmbeddingConfig {
    pub model: String,       // デフォルト: "text-embedding-3-small"
    pub dimensions: usize,   // デフォルト: 1536
}

pub struct LocalEmbeddingConfig {
    pub model: String,         // デフォルト: "jina-embeddings-v5-text-small"
    pub quantization: String,  // デフォルト: "F16"
}
```

---

## エラー

### `LlmProviderError`

ライブラリ境界で `LlmProvider` の実装によって返されるエラーです。

```rust
pub enum LlmProviderError {
    Auth(String),
    RateLimit(String),
    Network(String),
    Truncated { reason: String, partial_chars: usize },
    ContentFilter(String),
    Provider(String),
}
```

| バリアント | 意味 |
|---|---|
| `Auth(String)` | プロバイダーが認証情報を拒否した（通常 HTTP 401/403）。 |
| `RateLimit(String)` | プロバイダーがこのリクエストをスロットリングした（通常 HTTP 429）。 |
| `Network(String)` | ネットワークレベルの失敗（接続拒否、DNS、TLS、読み取りタイムアウト）でリクエストが完了しなかった — レスポンス*ありの* HTTP レベルエラー用の `Provider` とは異なる。 |
| `Truncated { reason, partial_chars }` | 設定されたトークン制限に達したため（`finish_reason=length`）、レスポンスが途中で切られた。`partial_chars` は切られる前に返されたテキストの量で、診断に役立つ。 |
| `ContentFilter(String)` | プロバイダーがレスポンスをブロックした（通常 `finish_reason=content_filter`）。使用可能なテキストは返されなかった。 |
| `Provider(String)` | 上記のカテゴリに当てはまらないプロバイダー固有のエラーのキャッチオール。 |

`map_openai_error`（クレート内部）は、`async_openai::error::OpenAIError` を HTTP ステータスコードに基づいてこれらのバリアントにマッピングします：401/403 → `Auth`、429 → `RateLimit`、その他の API エラー → `Provider`、トランスポート／ストリームエラー → `Network`。

### `EmbeddingError`

```rust
pub enum EmbeddingError {
    Init(String),
    Provider(String),
    EmptyInput,
}
```

| バリアント | 意味 |
|---|---|
| `Init(String)` | 埋め込みモデルの初期化に失敗した（例：GGUF 読み込みエラー）。トランスポート／API エラー用の `Provider` とは異なる。 |
| `Provider(String)` | プロバイダーが不正な形式または空のレスポンスを返した、あるいはトランスポートエラー（HTTP 4xx/5xx、ネットワーク障害）によってリクエストが妨げられた。 |
| `EmptyInput` | 与えられたテキストが空または空白のみである。プロバイダーはこれを埋め込むことを拒否する — ゼロベクトルを返すとコサイン類似度が未定義になり、ストアを静かに汚染してしまうため。 |

---

## 組み込み実装

### `OpenAiProvider`

`async-openai` を介して OpenAI 互換の HTTP API（OpenAI、Azure、ローカルプロキシ）と通信します。ストリーミングと構造化 JSON 出力をサポートします。

```rust
pub struct OpenAiProvider { /* 非公開 */ }

impl OpenAiProvider {
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self;
}
```

`new` は基盤となる `async_openai::Client` を即座に構築します。`base_url` が空でない場合、クライアントのデフォルトの API ベースを上書きします。

### `OpenAiProviderFactory`

```rust
pub struct OpenAiProviderFactory;

impl LlmProviderFactory for OpenAiProviderFactory { /* ... */ }
```

`"openai-compatible"` という名前で登録されます。その `create_provider` は、渡された `EneConfig` から `ProviderConfig` を読み取り、ベース URL と API キーを解決し、`OpenAiProvider` を構築します。

### `CloudEmbeddingProvider`

クラウドの埋め込みAPI（OpenAI 互換）に委譲する `EmbeddingProvider` です。ローカル GGUF プロバイダーよりも高いスループットが必要な本番用途に適しています。

```rust
pub struct CloudEmbeddingProvider { /* 非公開 */ }

impl CloudEmbeddingProvider {
    pub fn new(
        base_url: &str,
        api_key: &str,
        embedding_model: &str,
        embedding_dimensions: usize,
        query_prefix: Option<String>,
    ) -> Self;

    pub fn with_hyde_model(self, model: String) -> Self;
}
```

| メソッド | 説明 |
|---|---|
| `new(...)` | クライアントを構築し、`embedding_model`/`embedding_dimensions`/`query_prefix` を保持する。`query_prefix`（設定されている場合）は `Query` kind のテキストにのみ、正確に1回だけ前置される — 他の kind には決して前置されず、二重に前置されることもない（`embed_query` は自身でプレフィックスを再適用するのではなく `embed` を呼び出す）。 |
| `with_hyde_model(model)` | ビルダーメソッド。設定すると、`hyde()` はクエリをそのままエコーバックする代わりに、指定されたチャットモデルを呼び出して実際の仮想文書を生成する。 |

`embed_batch` は、API が入力数と異なる数の埋め込みを返した場合、静かに切り捨てるのではなく `EmbeddingError::Provider` で明確に失敗します。

### `HybridRerankProvider`

主要な `EmbeddingProvider` をラップし、その上に**任意の** LLM ベースの HyDE とリランクステップを追加します。

```rust
pub struct HybridRerankProvider { /* 非公開 */ }

impl HybridRerankProvider {
    pub fn new(embedder: Arc<dyn EmbeddingProvider>) -> Self;

    pub fn with_llm(
        self,
        hyde_llm: Option<Arc<dyn LlmProvider>>,
        rerank_llm: Option<Arc<dyn LlmProvider>>,
    ) -> Self;
}
```

| メソッド | 説明 |
|---|---|
| `new(embedder)` | すべての `embed`/`embed_query`/`embed_batch` 呼び出しに `embedder` をラップする。`hyde()` と `rerank()` は、LLM が接続されるまでトレイトのデフォルト（クエリのエコー／コサイン類似度）にフォールバックする。 |
| `with_llm(hyde_llm, rerank_llm)` | HyDE 生成とリランクスコアリング用に別々の LLM プロバイダーを接続する。それぞれ独立して任意 — そのタスクのデフォルトフォールバックを維持するには `None` を渡す。この2つが意図的に別々の `Arc<dyn LlmProvider>` インスタンスになっているのは（共有プロバイダー + モデル名のペアではない）、モデル名だけでは、すでに構築済みのプロバイダーがワイヤー上で実際に話しているモデルを上書きできないためである。 |

`rerank_llm` が設定されている場合、`rerank()` はすべての候補を含むプロンプトを LLM に送り、`candidates` と同じ順序で `0.0..=1.0` のスコアの JSON 配列（`{"scores": [...]}`）を要求します。不正な形式または長さの異なるレスポンスは、サイレントな全ゼロフォールバックではなく、型付きの `EmbeddingError::Provider` になります。`has_reranker()` は、`rerank_llm` が `Some` の場合に正確に `true` を返します。

---

## 使用例

### チャットターンのストリーミング

```rust,no_run
use ene_provider::{LlmMessage, LlmProvider, UserMessagePart};
use futures::StreamExt;

async fn stream_reply(provider: &dyn LlmProvider) -> Result<(), Box<dyn std::error::Error>> {
    let messages = vec![
        LlmMessage::System { content: "You are a helpful assistant.".into() },
        LlmMessage::User {
            parts: vec![UserMessagePart::Text {
                text: "What is the capital of France?".into(),
            }],
        },
    ];

    let mut stream = provider.create_chat_stream(&messages, &[]).await?;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if let Some(delta) = chunk.text_delta {
            print!("{delta}");
        }
    }
    println!();
    Ok(())
}
```

### クエリの埋め込み

```rust,no_run
use ene_provider::EmbeddingProvider;

async fn embed_query(provider: &dyn EmbeddingProvider) -> Result<Vec<f32>, ene_provider::EmbeddingError> {
    provider.embed_query("recent conversations about Rust").await
}
```

### 構造化された補完

```rust,no_run
use ene_provider::{LlmMessage, LlmProvider};

async fn summarize(provider: &dyn LlmProvider, messages: &[LlmMessage]) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" },
            "key_facts": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["summary", "key_facts"]
    });

    let json_str = provider.chat_completion(messages, Some(schema)).await?;
    Ok(serde_json::from_str(&json_str)?)
}
```

### ファクトリの登録と利用

```rust,no_run
use ene_provider::{LlmProviderRegistry, OpenAiProviderFactory};
use std::sync::Arc;

fn setup(config: &ene_config::EneConfig) -> Result<Box<dyn ene_provider::LlmProvider>, ene_provider::LlmProviderError> {
    LlmProviderRegistry::register(Arc::new(OpenAiProviderFactory));
    LlmProviderRegistry::create_provider("openai-compatible", config)
}
```

---

## 関連項目

- [`ene-core`](./ene-core.md) — プロバイダーを駆動するランタイム
- [`ene-embedding`](./ene-embedding.md) — ローカル GGUF 埋め込みプロバイダー（`EmbeddingProvider` を実装）
- [`ene-config`](./ene-config.md) — `EneConfig`、`define_config!`
- [`ene-memory`](./ene-memory.md) — ベクトル検索のために埋め込みを利用する
