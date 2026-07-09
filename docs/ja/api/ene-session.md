# `ene-session` — APIリファレンス

> **クレート:** `ene-session`
> **役割:** 会話セッション管理、ストリーミングテキスト処理、セッション境界検出、およびキャラクター駆動の表情状態管理。

---

## 概要

`ene-session` は単一チャットセッションの可変な会話状態を保持します。履歴を追跡し、ストリーミングデルタを組み立て、特殊トークン（感情マーカー）を解析し、セッション境界の分割をスコアリング・実行し、キャラクターアバターの表情ヒステリシスを追跡します。

`ConversationSession` は `ene-core` 内の `EneActor` によって保持・駆動されます。セッション分割の処理はすべて非同期であり、バックグラウンドの `tokio` タスクとして実行されるため、ターンループを一切ブロックしません — アクターは完了をポーリングし、結果が到着した時点で適用します。

---

## `ConversationSession`

中心となる型。会話履歴、メモリコンテキスト、ストリーミング表示状態、表情追跡を組み合わせます。

```rust
pub struct ConversationSession {
    pub(crate) history: ConversationHistory,
    pub display: DisplayState,
    pub memory: MemoryContext,
    pub(crate) state: SessionState,
    pub character_card: Option<CharacterCardV3>,
    // current_card_path: String（プライベート）
}
```

### 構築 & 初期化

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `new` | `pub fn new() -> Self` | 履歴なし・新しい `SessionId` を持つ新規セッションを作成する。 |
| `init_memory` | `pub fn init_memory(&mut self, store: Arc<MemoryStore>, embedder: Arc<dyn EmbeddingProvider>)` | メモリストアと埋め込みプロバイダをアタッチする。メモリを使う操作の前に必ず呼び出す必要がある。 |
| `load_card` | `pub fn load_card(&mut self, path: &str) -> Result<Vec<ResolvedExpression>, EneSessionError>` | ディスクからキャラクターカードを読み込み、解決済みの表情アセットを返す。 |

### 履歴管理

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `add_user_message` | `pub fn add_user_message(&mut self, input: &str)` | ユーザーターンを履歴に追加する。 |
| `add_assistant_message` | `pub fn add_assistant_message(&mut self, text: &str)` | アシスタントターンを履歴に追加する。 |
| `history` | `pub fn history(&self) -> &[(Role, String)]` | メモリ上の会話履歴全体を返す。 |
| `trim_history_keep_last` | `pub fn trim_history_keep_last(&mut self, keep: usize)` | メモリ上の履歴を最後の `keep` 件のみに切り詰める。 |

### ストリーミング

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `process_delta` | `pub fn process_delta(&mut self, chunk: &str) -> (Vec<String>, Vec<String>)` | ストリーミングチャンクを表示バッファに投入し、テキストデルタと特殊トークン（例: `<|emo:happy|>`）に分割する。`(text_deltas, special_tokens)` を返す。 |
| `finalize_response` | `pub fn finalize_response(&mut self) -> Option<String>` | 残っているトークンキャリーをフラッシュし、バッファ済みテキストをアシスタントメッセージとしてコミットし、残留するトークン断片を返す。 |
| `reset_display_buffer` | `pub fn reset_display_buffer(&mut self)` | 履歴に影響を与えずにストリーミング表示バッファをクリアする。 |

### セッションライフサイクル

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `reset_session` | `pub fn reset_session(&mut self) -> SessionId` | 履歴、表示状態、ターン数をリセットし、新しい `SessionId` を生成して返す。 |
| `session_id` | `pub fn session_id(&self) -> &SessionId` | 現在のセッションの一意なID。 |
| `session_started_at` | `pub fn session_started_at(&self) -> DateTime<Utc>` | 現在のセッションが開始した時刻。 |
| `session_elapsed_minutes` | `pub fn session_elapsed_minutes(&self) -> i64` | セッション開始からの経過分数。 |
| `current_turn_count` | `pub fn current_turn_count(&self) -> usize` | このセッションで完了したターン数。 |
| `last_message_time` | `pub fn last_message_time(&self) -> Option<DateTime<Utc>>` | 直近のメッセージの時刻。 |
| `card_name` | `pub fn card_name(&self) -> &str` | 読み込まれたキャラクターカードの名前。 |

### 埋め込み

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `set_pending_embedding` | `pub fn set_pending_embedding(&mut self, embedding: Vec<f32>)` | 次のターンのメモリ検索に使う、現在のユーザー入力の埋め込みを保存する。 |
| `set_last_input_embedding` | `pub fn set_last_input_embedding(&mut self, embedding: Vec<f32>)` | トピック変化の境界検出に使う埋め込みを保存する。 |

### ターン追跡

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `record_user_input` | `pub fn record_user_input(&mut self)` | `current_turn_count` を増やし `last_message_time` を設定する。メモリログへの永続化は**一切行わない** — 生ログの記録は `execute_split` の内部で別途行われる。 |
| `record_assistant_response` | `pub fn record_assistant_response(&mut self)` | `record_user_input` と同じ記録処理を、アシスタントのターン後に呼び出す。 |

### 表情追跡

キャラクターの表情**アービター**を、セッション内ヒステリシスによって支えます。これにより、視覚的に似た状態間でターンごとに表情がちらつくことを防ぎます。

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `expression_elapsed` | `pub fn expression_elapsed(&self) -> Option<std::time::Duration>` | 最後の表情変更からの経過時間。このセッションでまだ変更が記録されていない場合は `None`。 |
| `record_expression_change` | `pub fn record_expression_change(&mut self, name: &str)` | ヒステリシス追跡のために、新しく解決された表情とそのタイムスタンプを記録する。 |
| `last_resolved_expression` | `pub fn last_resolved_expression(&self) -> &str` | このセッションの最後に解決された表情名（まだ無い場合は空文字列）。 |
| `expression_context` | `pub fn expression_context<'a>(&'a self, affect: &'a AffectState) -> (Cow<'a, str>, Option<std::time::Duration>)` | アービターのヒステリシス用に `(前の表情, 経過時間)` を返す。セッション内トラッカーが空の場合（例: 再起動直後）は、永続化された `AffectState::last_expression` / `updated_at` にフォールバックする。 |

### セッション分割との連携

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `prepare_split_input` | `pub fn prepare_split_input(&self, config: &EneConfig, user_input: &str, user_name: &str, provider: Arc<dyn LlmProvider>) -> Option<SplitTaskInput>` | バックグラウンドで分割タスクを実行するために必要なすべてのデータを収集する。メモリが初期化されていない場合は `None` を返す。 |
| `mark_split_pending` | `pub fn mark_split_pending(&mut self)` | 進行中の分割のスナップショット境界として、現在の履歴の長さを記録する。 |
| `is_split_pending` | `pub fn is_split_pending(&self) -> bool` | 現在、分割スナップショット境界が記録されているかどうか。 |
| `apply_split_result` | `pub fn apply_split_result(&mut self, split: &SplitResult)` | 完了した分割を適用する: スナップショット境界（`split.snapshot_len`）まで履歴を切り詰め、トリガーとなったターンと分割実行中に追加されたものを保持する。`split.new_session_id` へローテーションし、保留マーカーをクリアする。 |
| `clear_split_pending` | `pub fn clear_split_pending(&mut self)` | 結果を適用せずに保留マーカーをクリアする — `EneSessionError::SplitNotNeeded` のような致命的でない分割エラーに使用される。 |
| `apply_pending_split` | `pub fn apply_pending_split(&mut self, pending_split: &mut Option<PendingSplitTask>) -> Option<Result<SplitResult, EneSessionError>>` | `poll_split_result` を介してバックグラウンド分割タスクをポーリングする。成功時はセッションをリセットし（**全履歴をクリア**）、新しいセッションIDを採用する。進行中のメッセージを保持する必要がある場合は、下記の注記の通り `mark_split_pending` + `apply_split_result` を推奨する。 |

> **注記:** `apply_pending_split` と `apply_split_result` は、`SplitResult` を消費するための2つの異なる戦略を実装しています。`apply_pending_split` は無条件に `reset_session()` を呼び出し、全履歴を破棄します。一方 `apply_split_result` は `snapshot_len` までのみ切り詰め、分割のスナップショットが取られた後に届いたメッセージを保持します。新しい統合では `mark_split_pending` / `apply_split_result` の組を推奨します。

---

## セッションを支える補助的な型

### `ConversationHistory`

```rust
/// 自動トリミング機能を持つ会話履歴を管理する。
pub struct ConversationHistory {
    pub conversation_history: Vec<(Role, String)>,
    pub max_history_turns: usize,
}
```

`max_history_turns` を超えると、LLMに送られるメモリ上のコンテキストから最も古いターンがトリミングされます（`ene-memory` 内の永続的な会話ログには影響しません）。

### `DisplayState`

```rust
/// 現在の表示バッファと部分的なトークンキャリーオーバーを保持する。
#[derive(Clone, Debug, Default)]
pub struct DisplayState {
    pub display_buffer: String,
    pub token_carry: String,
}
```

### `MemoryContext`

```rust
pub struct MemoryContext {
    pub memory_store: Option<Arc<MemoryStore>>,
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    pub session_id: SessionId,
    pub session_started_at: DateTime<Utc>,
    pub pending_embedding: Option<Vec<f32>>,
    /// 最後に同期したCCv3キャラクターメモリインデックスのキャッシュされたハッシュ。
    pub ccv3_memory_hash: Option<u64>,
}
```

### `SessionState`（プライベートフィールド、アクセサ経由で公開）

```rust
#[derive(Clone, Debug, Default)]
pub struct SessionState {
    pub last_input_embedding: Option<Vec<f32>>,
    pub last_message_time: Option<DateTime<Utc>>,
    pub current_turn_count: usize,
    /// 分割が進行中の間、スナップショットが取られた時点での履歴の長さ。
    pub pending_split_snapshot_len: Option<usize>,
    pub last_resolved_expression: String,
    pub last_expression_changed_at: Option<DateTime<Utc>>,
}
```

---

## セッション境界の型

### `SessionBoundary`

```rust
#[derive(Debug)]
pub enum SessionBoundary {
    /// 分割せずにセッションを継続すべき。
    Continue,
    /// 指定された理由でセッションを分割すべき。
    Split(SplitReason),
}
```

### `SplitReason`

5つのバリアントがあり、境界検出器のトリガーを可観測性やプロンプト向けメッセージのために区別します:

```rust
#[derive(Debug, Clone)]
pub enum SplitReason {
    /// 非活動タイムアウトを超えた。
    Timeout { elapsed_minutes: u64 },
    /// トピックが大きく変化した（埋め込み類似度が低い）。
    TopicChange { similarity: f32 },
    /// コンテキスト長の圧迫 — 履歴が上限に近づいている。
    ContextPressure { context_ratio: f32 },
    /// 複数要因にわたる高い複合スコアが分割を発生させた。
    Composite { score: f32 },
    /// ユーザーまたはシステムが手動での分割を要求した。
    Manual,
}
```

`SplitReason` は、人間が読めるログ/UIメッセージ用に `Display` を実装しています。

### `SplitResult`

```rust
#[derive(Debug, Clone)]
pub struct SplitResult {
    pub reason: SplitReason,
    pub summary: String,
    pub key_facts: Vec<KeyFact>,
    pub new_session_id: SessionId,
    /// サマライザーに渡されたスナップショットに含まれていた履歴エントリ数。
    /// `apply_split_result` はインデックス `snapshot_len - 1` より前のエントリを
    /// 破棄し、残りを保持するため、分割実行中に届いたメッセージは保持される。
    pub snapshot_len: usize,
}
```

---

## セッション境界の検出とスコアリング

### `check_boundary`

```rust
pub async fn check_boundary(
    last_input_embedding: Option<&Vec<f32>>,
    last_message_time: Option<DateTime<Utc>>,
    current_turn_count: usize,
    history_len: usize,
    settings: &SessionConfig,
    user_input: &str,
    embedder: &dyn EmbeddingProvider,
) -> SessionBoundary
```

非同期であり、`SessionBoundary` を直接返します（`Result` ではない）。`user_input` を埋め込み、`compute_split_score` によって複合分割スコアを計算し、以下の場合に `SessionBoundary::Continue` を返します:
- `settings.auto_split` が無効、または
- `settings.min_turns_before_split` 未満のターン数しか発生していない、または
- 複合スコアが `settings.split_weights.threshold` 未満。

### `compute_split_score` / `SplitScore`

```rust
#[must_use]
pub fn compute_split_score(
    time_elapsed_minutes: f64,
    topic_similarity: Option<f32>,
    context_ratio: f32,
    turn_count: usize,
    config: &SessionConfig,
) -> SplitScore
```

```rust
/// 診断用に分解された分割スコアの構成要素。
#[derive(Debug, Clone)]
pub struct SplitScore {
    /// 加重合計スコア。`config.split_weights.threshold` に達すると分割が発生する。
    pub total: f32,
    pub time_component: f32,
    pub topic_component: f32,
    pub context_component: f32,
    pub turn_component: f32,
}
```

計算式: `total = time * time_factor + topic * topic_distance + context * context_pressure + turn_count * turn_factor`。ここで `topic_distance = 1 − cosine_similarity`、`context_pressure = history_len / max_history` です。

### `SplitScoreWeights`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SplitScoreWeights {
    /// デフォルト 0.35。
    pub time: f32,
    /// デフォルト 0.40。
    pub topic: f32,
    /// デフォルト 0.20。
    pub context: f32,
    /// デフォルト 0.05。
    pub turn_count: f32,
    /// これを超えると分割が発生するスコア閾値。デフォルト 0.65。
    pub threshold: f32,
}
```

### `execute_split`

```rust
pub async fn execute_split(
    history: &[(Role, String)],
    session_id: &str,
    card_name: &str,
    user_name: &str,
    store: &Arc<MemoryStore>,
    embedder: &Arc<dyn EmbeddingProvider>,
    provider: &dyn LlmProvider,
    reason: SplitReason,
) -> Result<SplitResult, EneSessionError>
```

分割パイプライン全体を実行します: `MemoryStore::insert_log` を介して会話履歴を `conversation_logs` に永続化し、LLM（`ene_memory::summarize_conversation`）で要約し、要約とキーファクトを永続化し、新しい `SessionId` を生成します。

### バックグラウンドタスクのオーケストレーション

| 関数 | シグネチャ | 説明 |
|----------|-----------|-------------|
| `spawn_split_task` | `pub fn spawn_split_task(pending_split: &mut Option<PendingSplitTask>, input: SplitTaskInput)` | `check_boundary` の後に `execute_split` を呼び出す `tokio` タスクを生成する（もしくは `Err(EneSessionError::SplitNotNeeded)` で完了する）。分割が既に保留中の場合は何もしない。 |
| `poll_split_result` | `pub fn poll_split_result(pending_split: &mut Option<PendingSplitTask>) -> Option<Result<SplitResult, EneSessionError>>` | タスクのoneshotチャネルに対する非ブロッキングな `try_recv`。実行中は `None`。`Empty` の場合はタスクを再保存する。 |
| `generate_session_id` | `pub fn generate_session_id() -> SessionId` | `SessionId::from(format!("session_{}", Uuid::new_v4()))`。 |

```rust
pub struct SplitTaskInput {
    pub last_input_embedding: Option<Vec<f32>>,
    pub last_message_time: Option<DateTime<Utc>>,
    pub current_turn_count: usize,
    /// 現在のメモリ上の履歴の長さ（ターン数ではなく個々のメッセージ数）。
    pub history_len: usize,
    pub user_input: String,
    pub session_config: SessionConfig,
    pub provider: Arc<dyn LlmProvider>,
    pub history: Vec<(Role, String)>,
    pub session_id: SessionId,
    pub card_name: CardName,
    pub user_name: String,
    pub store: Arc<MemoryStore>,
    pub embedder: Arc<dyn EmbeddingProvider>,
}

pub struct PendingSplitTask {
    // rx: oneshot::Receiver<Result<SplitResult, EneSessionError>>（プライベート）
}
```

### `embed_session_messages`

```rust
pub async fn embed_session_messages(
    embedder: &dyn EmbeddingProvider,
    history: &[(Role, String)],
) -> Result<Vec<f32>, EneSessionError>
```

すべての `User`/`Assistant` メッセージを個別に埋め込み（空またはテキストでないターンはスキップ）、それらを平均化ではなく**maxプーリング**によって統合します — 全メッセージベクトルにわたって次元ごとに最大値を取り、その後L2正規化を行います。maxプーリングは次元ごとに最も強い信号を採用するため、情報量の少ないターン（挨拶など）がセッションの意味的特徴を薄めることを防ぎます。各メッセージを個別に埋め込むことで、長いセッションで埋め込みモデルの `max_length` 制限に達することも回避できます。履歴が空の場合はゼロベクトルを返します。

---

## 特殊トークンの解析

LLMの出力ストリームには、UI側のエフェクト（アバターの表情変化など）を駆動する特殊トークン（現時点では感情マーカー）が含まれることがあります。

### `split_text_and_special_tokens`

```rust
pub fn split_text_and_special_tokens(
    carry: &mut String,
    chunk: &str,
) -> (Vec<String>, Vec<String>)
```

ストリーミングチャンクを、プレーンテキストのデルタと `<|...|>` 形式の完全な特殊トークンに分割します。`carry` は不完全なトークン（または末尾に孤立した `<`）をチャンク境界をまたいで保持します — 同じストリームに対しては、呼び出しごとに同じ `carry` バッファを渡してください。

### `extract_emotion_from_token`

```rust
#[must_use]
pub fn extract_emotion_from_token(token: &str) -> Option<String>
```

感情トークンは **`<|emo:name|>`** という形式を使用します（`emo` プレフィックスは大文字小文字を区別しない）。有効な感情トークンに対しては `Some(name)`（小文字化・トリム済み）を返し、それ以外（他の種類のトークン、空の名前、プレーンテキスト）に対しては `None` を返します。

```rust
assert_eq!(extract_emotion_from_token("<|emo:happy|>"), Some("happy".to_string()));
assert_eq!(extract_emotion_from_token("<|act:wave|>"), None);
```

---

## 型安全なIDラッパー

### `SessionId` / `CardName`

内部の `define_id_type!` マクロによって生成される `String` のnewtypeラッパーで、セッションIDとカード名を型レベルで誤って混同することを防ぎます。

```rust
pub struct SessionId(String);
pub struct CardName(String);

impl SessionId /* および CardName */ {
    pub fn new(id: impl Into<String>) -> Self;
    pub fn as_str(&self) -> &str;
    pub fn into_inner(self) -> String;
}

impl From<String> for SessionId /* および CardName */ { /* ... */ }
impl From<&str> for SessionId /* および CardName */ { /* ... */ }
```

両方とも `Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize` を導出し、`Display` を実装しています。

---

## 設定

### `SessionConfig`

```rust
pub struct SessionConfig {
    /// 自動セッション分割を有効にするか。デフォルト `true`。
    pub auto_split: bool,
    /// 時間要因がフルに寄与するまでの分数。デフォルト `30`。
    pub timeout_minutes: u64,
    /// メモリ上の履歴ウィンドウに保持する最大会話ターン数。デフォルト `20`。
    pub max_history_turns: usize,
    /// 分割が発生するために必要な最小ターン数。デフォルト `3`。
    pub min_turns_before_split: usize,
    /// プロンプトに注入する最大要約数。デフォルト `3`。
    pub recall_limit: usize,
    /// `search_summaries` が使用する埋め込み類似度閾値。デフォルト `0.5`。
    pub similarity_threshold: f32,
    pub split_weights: SplitScoreWeights,
    pub summarization: SummarizationConfig,
}
```

`ene_config::define_config!` を通じて `session` 設定キー配下でロードされます。

### `SummarizationConfig`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct SummarizationConfig {
    /// サマライズに使用するモデル。空の場合はチャットモデルにフォールバックする。
    pub model: String,
    /// サマライズに使用するベースURL。空の場合はチャットのベースURLにフォールバックする。
    pub base_url: String,
}

impl SummarizationConfig {
    #[must_use]
    pub fn resolve_summarization_model(&self, fallback_model: &str) -> String;
    pub fn resolve_summarization_base_url(&self, fallback_url: &str) -> Result<String, ene_config::ConfigError>;
}
```

---

## エラー: `EneSessionError`

```rust
#[derive(Error, Debug)]
pub enum EneSessionError {
    /// 分割評価の結果、分割は（まだ）不要と判断された。
    #[error("Split not needed")]
    SplitNotNeeded,
    /// 分割タスクのoneshotチャネルが予期せず閉じた。
    #[error("Task channel closed")]
    ChannelClosed,
    #[error(transparent)]
    Config(#[from] ene_config::EneConfigError),
    #[error("Embedding error: {0}")]
    Embedding(String),
    #[error(transparent)]
    Memory(#[from] ene_memory::EneMemoryError),
}
```

---

## 再エクスポート

`ene-session` はクレートルートでいくつかの便利なアイテムを再エクスポートしており、その中には `ene-common` からの `Truncate` も含まれます:

```rust
pub use ene_common::truncate::Truncate;
```

その他の再エクスポート: `SessionConfig`、`SplitScoreWeights`、`SummarizationConfig`；`CharacterAsset`、`CharacterCardData`、`CharacterCardV3`、`ExpressionDefinition`、`ResolvedExpression`、`expand_cbs_macros`、`resolve_expressions`（`ene-config` から）；`Role`（`ene-provider` から）；`EneSessionError`；`ConversationSession`；`PendingSplitTask`、`SessionBoundary`、`SplitReason`、`SplitResult`、`SplitScore`、`SplitTaskInput`、`check_boundary`、`compute_split_score`、`execute_split`、`generate_session_id`、`poll_split_result`、`spawn_split_task`；`extract_emotion_from_token`、`split_text_and_special_tokens`；`CardName`、`SessionId`。

`embed_session_messages`、`ConversationHistory`、`DisplayState`、`SessionState` は公開されていますが、クレートルートでは再エクスポートされておらず、それぞれのモジュール（`session_split::embed_session_messages`、`session::*`）経由でのみアクセス可能です。

---

## 使用例

```rust,no_run
use ene_session::ConversationSession;

fn main() {
    let mut session = ConversationSession::new();

    // ストリーミングターンをシミュレートする。
    session.add_user_message("Tell me a joke");
    session.record_user_input();

    let chunks = ["Why did the ", "chicken cross", " the road?", "<|emo:happy|>"];
    for chunk in &chunks {
        let (text, tokens) = session.process_delta(chunk);
        for t in text {
            print!("{t}");
        }
        for t in tokens {
            if let Some(emotion) = ene_session::extract_emotion_from_token(&t) {
                session.record_expression_change(&emotion);
                eprintln!("[emotion: {emotion}]");
            }
        }
    }

    // ストリーム終了後に確定する。
    if let Some(full_response) = session.finalize_response() {
        session.add_assistant_message(&full_response);
        session.record_assistant_response();
    }

    println!("Turn count: {}", session.current_turn_count());
    println!("Last expression: {}", session.last_resolved_expression());
}
```

---

## 関連項目

- [`ene-core`](./ene-core.md) — `EneActor` を通じてセッションを駆動し、`apply_pending_split` をポーリングして表情変化を適用する
- [`ene-memory`](./ene-memory.md) — 分割時に作成される要約とログを保存する。`expression_context` の `AffectState` フォールバックを支える
- [`ene-provider`](./ene-provider.md) — `LlmProvider`、`EmbeddingProvider`、`Role`、`LlmMessage`
- [`ene-config`](./ene-config.md) — `SessionConfig`、キャラクターカードの型、CBSマクロ展開
- [`ene-common`](./ene-common.md) — `Truncate` ユーティリティトレイト
