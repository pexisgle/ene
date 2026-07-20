# `ConversationSession` および会話セッション仕様

このドキュメントでは、Ene の会話セッション状態のライフサイクル管理、インメモリ履歴保持、キャラクターカードの動的ロード、およびインライン表情モーション特殊トークン（`<|perf:...|>`）の解析処理について定義します。

---

## 1. 構造体の定義と主要な会話セッションメソッド

### `ConversationSession` (パブリック / 構造体)
インメモリの対話セッション状態とリソースを管理するコア構造体です：
*   `character_card: Option<CharacterCardV3>`: パースされたSilleyTavern形式のキャラクターカード。
*   `history: Vec<HistoryEntry>`: 会話履歴のスレッド配列。
*   `display_buffer: String`: 特殊トークンを除去し、UI に表示するために待機しているクリーンな対話テキストのバッファ。
*   `session_id: SessionId`: セッションを表す一意の UUID。

#### `new`
*   **シグネチャ**: `pub fn new() -> Self`
*   **説明**: 会話履歴を空にして新しい UUID を持つセッションインスタンスを構築します。

#### `init_memory`
*   **シグネチャ**: `pub fn init_memory(&mut self, store: Arc<MemoryStore>, embedder: Arc<dyn EmbeddingProvider>)`
*   **説明**: SQLite 接続プールとベクトル埋め込みプロバイダをセッションインスタンスにバインドします。

#### `set_card`
*   **シグネチャ**: `pub fn set_card(&mut self, card: &CharacterCardV3) -> Vec<ResolvedExpression>`
*   **説明**: セッションにキャラクター設定情報を読み込み、カードに記述されている VRM 表情定義（表情別ブレンドシェイプ重み情報など）を解決して、解決された表情カタログを返します。

#### `load_card`
*   **シグネチャ**: `pub fn load_card(&mut self, path: &str) -> Result<Vec<ResolvedExpression>, super::error::EneSessionError>`
*   **説明**: 指定された画像ファイルパス（PNG または JSON）からキャラクターカードを動的に読み込み、アクターに登録します。

#### `add_user_message`
*   **シグネチャ**: `pub fn add_user_message(&mut self, input: &str)`
*   **説明**: ユーザーの発言を対話履歴（`session.history`）に追加します。

#### `add_assistant_message`
*   **シグネチャ**: `pub fn add_assistant_message(&mut self, text: &str)`
*   **説明**: アクターの完了発言を対話履歴に追加します。

#### `process_delta`
*   **シグネチャ**: `pub fn process_delta(&mut self, chunk: &str) -> (Vec<String>, Vec<String>)`
*   **説明**: LLM からのストリーミングデータフラグメントを受信し、純粋な対話用テキスト（`display_buffer` に追記）と表情アニメーション用の制御タグ配列に分離します。

#### `finalize_response`
*   **シグネチャ**: `pub fn finalize_response(&mut self) -> Option<String>`
*   **説明**: 進行中だったテキストストリーミングのバッファをクローズしてクリーニングし、アクターの発話テキストの全文を生成して返します。

#### `reset_display_buffer`
*   **シグネチャ**: `pub fn reset_display_buffer(&mut self)`
*   **説明**: ストリーミング処理用のテンポラリテキストバッファをクリアします。

#### `reset_session`
*   **シグネチャ**: `pub fn reset_session(&mut self) -> SessionId`
*   **説明**: 会話履歴、各種バッファ、以前の ID 情報などを全てクリアし、新たなセッション UUID を再生成してバインドします。

#### `set_pending_embedding`
*   **シグネチャ**: `pub fn set_pending_embedding(&mut self, embedding: Vec<f32>)`
*   **説明**: 後続のバックグラウンドメモリ書き込み処理のために、計算された最新の対話ベクトルを保留バッファにセットします。

#### `set_last_input_embedding`
*   **シグネチャ**: `pub fn set_last_input_embedding(&mut self, embedding: Vec<f32>)`
*   **説明**: ユーザーが発言した最新の発話テキストの埋め込みベクトル情報を記録します。

#### `record_user_input`
*   **シグネチャ**: `pub fn record_user_input(&mut self)`
*   **説明**: ユーザー発言を受信した際の各種セッション統計指標のメタデータを更新します。

#### `record_assistant_response`
*   **シグネチャ**: `pub fn record_assistant_response(&mut self)`
*   **説明**: アクターの発言が完了した際の指標データを更新します。

#### `expression_elapsed`
*   **シグネチャ**: `pub fn expression_elapsed(&self) -> Option<std::time::Duration>`
*   **説明**: アクターの現在の表情ブレンドシェイプが変更されてから経過した時間（Duration）を取得します。ヒステリシス時間の検証に使用されます。

#### `record_expression_change`
*   **シグネチャ**: `pub fn record_expression_change(&mut self, name: &str)`
*   **説明**: アクターの表情を変更し、タイムスタンプを更新します。

#### `last_resolved_expression`
*   **シグネチャ**: `pub fn last_resolved_expression(&self) -> &str`
*   **説明**: 現在アクターがとっている表情の名前を取得します。

#### `expression_context`
*   **シグネチャ**: `pub fn expression_context<'a>(&'a self, affect: &'a ene_store::AffectState) -> (Cow<'a, str>, Option<std::time::Duration>)`
*   **説明**: 現在の感情の PAD 座標と連動している表情情報、および経過時間のコンテキストをまとめて取得します。

#### `card_name`
*   **シグネチャ**: `pub fn card_name(&self) -> &str`
*   **説明**: 現在ロードされているキャラクターカードの識別名（またはデフォルトの代替名）を取得します。

#### `history`
*   **シグネチャ**: `pub fn history(&self) -> &[HistoryEntry]`
*   **説明**: 会話履歴のスレッド配列への直接アクセスを提供します。

#### `session_id`
*   **シグネチャ**: `pub const fn session_id(&self) -> &SessionId`
*   **説明**: アクティブなセッションの一意の ID を返します。

#### `session_started_at`
*   **シグネチャ**: `pub const fn session_started_at(&self) -> DateTime<Utc>`
*   **説明**: セッションが開始されたタイムスタンプを返します。

#### `current_turn_count`
*   **シグネチャ**: `pub const fn current_turn_count(&self) -> usize`
*   **説明**: 現在のセッションにおける総対話ターン数を返します。

#### `trim_history_keep_last`
*   **シグネチャ**: `pub fn trim_history_keep_last(&mut self, keep: usize)`
*   **説明**: 直近の `keep` 件のメッセージを残し、古い会話履歴スレッドデータを削除して切り捨てます。

#### `last_message_time`
*   **シグネチャ**: `pub const fn last_message_time(&self) -> Option<DateTime<Utc>>`
*   **説明**: セッション履歴の最後のメッセージが投稿されたタイムスタンプを取得します。

#### `session_elapsed_minutes`
*   **シグネチャ**: `pub fn session_elapsed_minutes(&self) -> i64`
*   **説明**: セッションが開始されてからの累積経過時間を分単位で算出します。

---

## 2. セッションの制御とID生成 (`session_split.rs`)

#### `generate_session_id`
*   **シグネチャ**: `pub fn generate_session_id() -> SessionId`
*   **説明**: ランダムに新しく一意の `SessionId` UUID を生成します。

---

## 3. インライン表情・パフォーマンス特殊トークンの解析 (`special_token.rs`)

感情更新アプレザルが無効化されている環境において、LLM がテキスト内に直接埋め込む表情変更用のタグ情報を検出・パースします：

#### `split_text_and_special_tokens`
*   **シグネチャ**: `pub fn split_text_and_special_tokens(carry: &mut String, chunk: &str) -> (Vec<String>, Vec<String>)`
*   **説明**: トークンストリームを受信し、ブラケットが閉じるまでの途中の未完成な文字列を `carry` にバッファしながら、対話テキスト（プレーンな表示テキスト）と完成した特殊パフォーマンスマークタグに分解して返します。

#### `strip_markers`
*   **シグネチャ**: `pub fn strip_markers(text: &str) -> String`
*   **説明**: テキスト全体から、すべてのインライン表情制御タグ（例: `<|perf:motion=wave|>`）を除去したクリーンな対話文字列を生成します。SQLite へのログ保存時に使用されます。

#### `parse_performance_marker`
*   **シグネチャ**: `pub fn parse_performance_marker(token: &str) -> Option<PerformanceCue>`
*   **説明**: 抽出された特殊トークン文字（例: `<|perf:expr=happy|>`) を解析し、表情かモーションかの指示種別およびブレンドシェイプ名、持続時間を含む `PerformanceCue` 構造体に変換します。

#### `strip_token_envelope`
*   **Signature**: `fn strip_token_envelope(token: &str) -> Option<&str>`
*   **Description**: 特殊マークアップタグの開始文字 `<|perf:` と終了文字 `|>` をトリミング除去します。

#### `parse_expr_marker`
*   **Signature**: `fn parse_expr_marker(rest: &str) -> Option<PerformanceCue>`
*   **Description**: 表情変更タグパラメータから、表情名、ブレンドウェイト比率、およびキープ持続時間を抽出します。

#### `parse_motion_marker`
*   **Signature**: `fn parse_motion_marker(rest: &str) -> Option<PerformanceCue>`
*   **Description**: モーション指定タグから、再生するモーションアニメーションアセット名およびターゲットボーンレイヤー情報を抽出します。
