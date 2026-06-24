# `ene-session` — APIリファレンス

> **クレート:** `ene-session`  
> **役割:** 会話セッション管理、ストリーミングテキスト処理、セッション境界の検出。

---

## 概要

`ene-session` は単一のチャットセッションにおける可変な会話状態を所有します。履歴の追跡、ストリーミングデルタの組み立て、特殊トークン（感情マーカーなど）の解析、そしてセッションをメモリにスプリットするタイミングの判断を担います。

`ConversationSession` は `ene-core` 内の `EneActor` が保持して駆動します。

---

## `ConversationSession`

中心となる型。会話履歴・メモリコンテキスト・ストリーミング状態を統合します。

```rust
pub struct ConversationSession { /* 非公開 */ }
```

### 構築と初期化

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `new` | `fn new() -> Self` | 履歴なし・新しい `SessionId` でフレッシュなセッションを作成します。 |
| `init_memory` | `fn init_memory(&mut self, store: Arc<MemoryStore>, embedder: Arc<dyn EmbeddingProvider>)` | メモリストアと埋め込みプロバイダーをセッションに紐付けます。メモリ操作の前に呼び出す必要があります。 |
| `load_card` | `fn load_card(&mut self, path: &str) -> Result<Vec<ResolvedExpression>, EneSessionError>` | ファイルシステムからキャラクターカードを読み込み、解決済み表情アセットを返します。 |

### 履歴管理

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `add_user_message` | `fn add_user_message(&mut self, input: &str)` | ユーザーのターンを履歴に追加します。 |
| `add_assistant_message` | `fn add_assistant_message(&mut self, text: &str)` | アシスタントのターンを履歴に追加します。 |
| `history` | `fn history(&self) -> &[(Role, String)]` | 完全な会話履歴をスライスで返します。 |

### ストリーミング

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `process_delta` | `fn process_delta(&mut self, chunk: &str) -> (Vec<String>, Vec<String>)` | ストリーミングチャンクをバッファに供給します。`(テキストデルタ, 特殊トークン)` を返します。 |
| `finalize_response` | `fn finalize_response(&mut self) -> Option<String>` | ストリームバッファをフラッシュし、完全なアシスタントメッセージを返します（存在する場合）。 |
| `reset_display_buffer` | `fn reset_display_buffer(&mut self)` | 履歴に影響を与えずにストリーミング表示バッファをクリアします。 |

### セッションライフサイクル

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `reset_session` | `fn reset_session(&mut self) -> SessionId` | 履歴をリセットして新しい `SessionId` を生成します。新しいIDを返します。 |
| `session_id` | `fn session_id(&self) -> &SessionId` | 現在のセッションの一意なID。 |
| `session_started_at` | `fn session_started_at(&self) -> DateTime<Utc>` | 現在のセッションの開始日時。 |
| `session_elapsed_minutes` | `fn session_elapsed_minutes(&self) -> i64` | セッション開始からの経過分数。 |
| `current_turn_count` | `fn current_turn_count(&self) -> usize` | このセッションで完了したターン数。 |
| `last_message_time` | `fn last_message_time(&self) -> Option<DateTime<Utc>>` | 最後のメッセージの日時。 |

### 埋め込み

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `set_pending_embedding` | `fn set_pending_embedding(&mut self, embedding: Vec<f32>)` | 次のターンのメモリ検索のために現在のユーザー入力の埋め込みを保存します。 |
| `set_last_input_embedding` | `fn set_last_input_embedding(&mut self, embedding: Vec<f32>)` | セッション境界検出のための埋め込みを保存します。 |

### 永続化

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `record_user_input` | `fn record_user_input(&mut self)` | ペンディングのユーザーターンを会話ログに永続化します。 |
| `record_assistant_response` | `fn record_assistant_response(&mut self)` | ペンディングのアシスタントターンを会話ログに永続化します。 |

### アクセサ

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `card_name` | `fn card_name(&self) -> &str` | 読み込まれているキャラクターカードの名前。 |
| `apply_pending_split` | `fn apply_pending_split(&mut self)` | 非同期で準備されていたペンディングのセッションスプリットをコミットします。 |
| `prepare_split_input` | `fn prepare_split_input(&self) -> SplitTaskInput` | 非同期スプリットタスクの実行に必要なデータを収集します。 |

---

## 内部構造体

### `ConversationHistory`

```rust
pub struct ConversationHistory {
    /// (ロール, 内容) ペアの順序付きリスト。
    pub conversation_history: Vec<(Role, String)>,

    /// アクティブなコンテキストウィンドウで保持する最大ターン数。
    pub max_history_turns: usize,
}
```

`max_history_turns` を超えると、古いターンはLLMに送信されるコンテキストから削除されます（永続ログからは削除されません）。

### `MemoryContext`

```rust
pub struct MemoryContext {
    /// アタッチされたメモリストア（メモリが無効な場合は None）。
    pub memory_store: Option<Arc<MemoryStore>>,

    /// クエリエンコード用の埋め込みプロバイダー。
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,

    /// 現在のセッションのID。
    pub session_id: SessionId,

    /// セッションの開始日時。
    pub session_started_at: DateTime<Utc>,

    /// 最新のユーザー入力の埋め込み（メモリ検索の準備済み）。
    pub pending_embedding: Option<Vec<f32>>,
}
```

---

## セッション境界型

### `SessionBoundary`

現在のターンで新しいセッションを開始すべきかどうかの評価結果です。

```rust
pub enum SessionBoundary {
    /// 現在のセッションを継続する。
    Continue,

    /// セッションをスプリットしてメモリサマリーを作成する。
    Split(SplitReason),
}
```

### `SplitReason`

```rust
pub enum SplitReason {
    /// セッションが長時間アイドル状態だった。
    Timeout { elapsed_minutes: u64 },

    /// 埋め込みの類似度が低く、トピックが大きく変化した。
    TopicChange { similarity: f32 },

    /// ユーザーまたはシステムが手動スプリットを実行した。
    Manual,
}
```

### `SplitResult`

```rust
pub struct SplitResult {
    /// スプリットがトリガーされた理由。
    pub reason: SplitReason,

    /// 完了したセッションのLLM生成サマリー。
    pub summary: String,

    /// セッションから抽出されたキーファクト。
    pub key_facts: Vec<KeyFact>,

    /// 次のセッション用に生成された新しいセッションID。
    pub new_session_id: SessionId,
}
```

---

## セッション境界関数

### `check_boundary`

```rust
pub fn check_boundary(
    last_embedding: Option<&Vec<f32>>,
    last_time: Option<&DateTime<Utc>>,
    turn_count: usize,
    settings: &SessionSettings,
    user_input: &str,
    embedder: &dyn EmbeddingProvider,
) -> SessionBoundary
```

受信したユーザーメッセージがセッションスプリットをトリガーすべきかを評価します。以下の2点をチェックします：
1. **タイムアウト:** 設定されたしきい値より長くセッションがアイドル状態だったか？
2. **トピック変化:** 直前ターンの埋め込みと新しい入力のコサイン類似度がしきい値を下回るか？

### `execute_split`

```rust
pub async fn execute_split(
    /* セッションデータ */
    ...
) -> Result<SplitResult, EneSessionError>
```

完全なスプリットパイプラインを実行します：LLMでセッション履歴を要約し、キーファクトを抽出し、サマリーをメモリストアに永続化し、新しい `SessionId` を生成します。

### `spawn_split_task`

```rust
pub fn spawn_split_task(
    pending: &mut Option<PendingSplitTask>,
    input: SplitTaskInput,
)
```

`execute_split` をバックグラウンドTokioタスクとして起動します。結果は後で `poll_split_result` で収集します。

### `poll_split_result`

```rust
pub fn poll_split_result(
    pending: &mut Option<PendingSplitTask>,
) -> Option<Result<SplitResult, EneSessionError>>
```

バックグラウンドスプリットタスクのノンブロッキングポーリング。タスクが完了したら `Some` を返し、まだ実行中なら `None` を返します。

### `generate_session_id`

```rust
pub fn generate_session_id() -> SessionId
```

新しい一意な `SessionId`（UUIDベース）を生成します。

### `embed_session_messages`

```rust
pub async fn embed_session_messages(
    embedder: &dyn EmbeddingProvider,
    history: &[(Role, String)],
) -> Result<Vec<f32>, EneSessionError>
```

履歴から User と Assistant のメッセージを個別に埋め込み、そのベクトル平均を 1 つのサマリー埋め込みとして返します。テキスト以外・空・非 User/Assistant のメッセージは除外されます。結果は境界検出のためのセッションのセマンティックな内容を表現します。

---

## 特殊トークン解析

LLMの出力ストリームには、UI効果を駆動する特殊トークン（例：`<|emotion:happy|>`）が含まれることがあります。

### `split_text_and_special_tokens`

```rust
pub fn split_text_and_special_tokens(
    carry: &mut String,
    chunk: &str,
) -> (Vec<String>, Vec<String>)
```

ストリーミングチャンクを特殊トークンに対して解析します。`carry` バッファはチャンク境界をまたぐ未完のトークンを保持します。

`(テキストデルタ, 特殊トークン)` を返します：
- `テキストデルタ`: 表示する通常テキストの断片。
- `特殊トークン`: このチャンクで見つかった完全な特殊トークン。

### `extract_emotion_from_token`

```rust
pub fn extract_emotion_from_token(token: &str) -> Option<String>
```

`token` が感情トークン形式（`<|emotion:名前|>`）にマッチする場合、`Some(名前)` を返します。それ以外は `None` を返します。

---

## 型安全なIDラッパー

### `SessionId`

```rust
pub struct SessionId(String);

impl SessionId {
    pub fn new(id: impl Into<String>) -> Self;
    pub fn as_str(&self) -> &str;
    pub fn into_inner(self) -> String;
}

impl From<String> for SessionId { ... }
impl From<&str> for SessionId { ... }
```

### `CardName`

```rust
pub struct CardName(String);

impl CardName {
    pub fn new(name: impl Into<String>) -> Self;
    pub fn as_str(&self) -> &str;
    pub fn into_inner(self) -> String;
}

impl From<String> for CardName { ... }
impl From<&str> for CardName { ... }
```

どちらも `String` 上のニュータイプラッパーで、セッションIDとカード名を型レベルで混同しないようにします。

---

## 再エクスポート

`ene-session` は利便性のために `ene-common` の `Truncate` トレイトを再エクスポートしています：

```rust
pub use ene_common::truncate::Truncate;
```

---

## 使用例

```rust
use ene_session::ConversationSession;

let mut session = ConversationSession::new();

// ストリーミングターンをシミュレートする
session.add_user_message("ジョークを教えて");

// チャンクが届くたびに供給する
let chunks = ["なぜニワトリは", "道路を渡っ", "たのか？"];
for chunk in &chunks {
    let (text, tokens) = session.process_delta(chunk);
    for t in text { print!("{}", t); }
    for t in tokens { eprintln!("[特殊: {}]", t); }
}

// ストリーム終了後にファイナライズする
if let Some(full_response) = session.finalize_response() {
    session.add_assistant_message(&full_response);
}

println!("ターン数: {}", session.current_turn_count());
```

---

## 関連項目

- [`ene-core`](./ene-core.md) — `EneActor` を通じてセッションを駆動する
- [`ene-memory`](./ene-memory.md) — スプリット時に作成されたサマリーを保存する
- [`ene-common`](./ene-common.md) — `Truncate` ユーティリティトレイト
