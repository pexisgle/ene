# `ene-ai` AI プロバイダおよびローカル GGUF 推論仕様

`ene-ai` クレートは、Ene のコアランタイムが呼び出す LLM（大規模言語モデル）のチャット生成および埋め込みベクトル化のための共通抽象化インターフェースを定義します。OpenAI 互換のクラウド Web API への接続を処理するほか、インプロセスで動作するローカル量子化 GGUF モデル（Llama.cpp 統合）の制御も担います。

---

## 1. 共通プロバイダインターフェース (Trait) とヘルパー

### `LlmProvider`
*   **定義**:
    ```rust
    #[async_trait]
    pub trait LlmProvider: Send + Sync {
        async fn create_chat_stream(
            &self,
            messages: &[LlmMessage],
            tools: &[ToolSpec],
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>, LlmProviderError>;
        async fn chat_completion(
            &self,
            messages: &[LlmMessage],
            json_schema: Option<serde_json::Value>,
        ) -> Result<String, LlmProviderError>;
        fn name(&self) -> &str;
    }
    ```

### `EmbeddingProvider`
*   **定義**:
    ```rust
    #[async_trait]
    pub trait EmbeddingProvider: Send + Sync {
        async fn embed_batch(
            &self,
            items: &[(&str, EmbeddingKind)],
        ) -> Result<Vec<Vec<f32>>, EmbeddingError>;
        fn dimensions(&self) -> usize;
        fn model_name(&self) -> &str;
    }
    ```

#### `collect_chat_completion`
*   **シグネチャ**: `pub async fn collect_chat_completion(provider: &dyn LlmProvider, messages: &[LlmMessage]) -> Result<String, LlmProviderError>`
*   **説明**: ストリーミングチャネルから返ってくるフラグメントトークンをすべて連結し、一つの完成した文字列応答を作成して返すヘルパー関数です。

#### `embed`
*   **シグネチャ**: `pub async fn embed<P: EmbeddingProvider + ?Sized>(provider: &P, text: &str, kind: EmbeddingKind) -> Result<Vec<f32>, EmbeddingError>`
*   **説明**: 単一のテキストブロックのベクトル埋め込みを計算するラッパーメソッドです。

#### `embed_query`
*   **シグネチャ**: `pub async fn embed_query<P: EmbeddingProvider + ?Sized>(provider: &P, text: &str) -> Result<Vec<f32>, EmbeddingError>`
*   **説明**: クエリ検索用のベクトル生成メソッドです。モデルの要求に応じて、必要な接頭辞（Prefix）などの文字列を自動で追加してベクトル化します。

#### `cosine_similarity`
*   **シグネチャ**: `pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32`
*   **説明**: 2つの float 配列ベクトル間のコサイン類似度（正規化ドット積）を計算します。

#### `LlmProviderFactory::register`
*   **シグネチャ**: `pub fn register(factory: Arc<dyn LlmProviderFactory>)`
*   **説明**: グローバルなランタイムレジストリに、新しい LLM プロバイダのファクトリハンドラを登録します。

#### `LlmProviderFactory::create_provider`
*   **シグネチャ**: `pub fn create_provider(name: &str, config: &ene_config::EneConfig) -> Result<Box<dyn LlmProvider>, LlmProviderError>`
*   **説明**: 設定に登録されている名前とパラメータに基づいて、対応する LLM プロバイダ実体を作成します。

---

## 2. OpenAI 互換プロバイダ実装 (`openai.rs`)

#### `build_openai_client`
*   **シグネチャ**: `pub(crate) fn build_openai_client(base_url: &str, api_key: &str) -> Client<OpenAIConfig>`
*   **説明**: 指定されたベース URL と API キーを使用して、OpenAI 互換エンドポイント用の接続クライアントオブジェクトを構築します。

#### `OpenAiProvider::new`
*   **シグネチャ**: `pub fn new(base_url: &str, api_key: &str, model: &str) -> Self`
*   **説明**: クラウド接続用の LLM プロバイダインスタンスを初期化します。

#### `OpenAiProvider::create_chat_stream`
*   **シグネチャ**: `async fn create_chat_stream(&self, messages: &[LlmMessage], tools: &[ene_plugin_proto::ToolSpec]) -> Result<Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>, LlmProviderError>`
*   **説明**: チャット要求を送信し、トークンが生成されるたびに非同期ストリームとして随時受け取るチャネルストリームオブジェクトを返します。

#### `OpenAiProvider::chat_completion`
*   **シグネチャ**: `async fn chat_completion(&self, messages: &[LlmMessage], json_schema: Option<serde_json::Value>) -> Result<String, LlmProviderError>`
*   **説明**: 感情分類処理用などに、JSON スキーマ制約をかけた構造化メッセージの単発完了レスポンスを要求します。

#### `OpenAiEmbeddingProvider::new`
*   **シグネチャ**: `pub fn new(base_url: &str, api_key: &str, embedding_model: &str, embedding_dimensions: usize, query_prefix: Option<String>) -> Self`
*   **説明**: クラウド接続用の埋め込みベクトルプロバイダを初期化します。

#### `OpenAiEmbeddingProvider::embed_batch`
*   **シグネチャ**: `async fn embed_batch(&self, items: &[(&str, EmbeddingKind)]) -> Result<Vec<Vec<f32>>, EmbeddingError>`
*   **説明**: 複数のテキストを一連のバッチとしてまとめて送信し、埋め込みベクトルを並列計算します。

#### `run_direct_sse_stream`
*   **シグネチャ**: `async fn run_direct_sse_stream(api_base: &str, api_key: &str, body: serde_json::Value, name_mapping: std::collections::HashMap<String, String>, tx: tokio::sync::mpsc::Sender<Result<LlmResponseChunk, LlmProviderError>>) -> Result<(), LlmProviderError>`
*   **説明**: カスタム API サーバー向けに、直接 Server-Sent Events (SSE) イベントを消費してパース処理する内部低レベルストリームハンドラです。

---

## 3. ローカル GGUF Llama.cpp プロバイダ (`local_llm/` および `llama_cpp/`)

#### `LocalLlamaCppProvider::load`
*   **シグネチャ**: `pub fn load(params: &LocalGgufLoadParams) -> Result<Self, LlmProviderError>`
*   **説明**: C++ Llama.cpp バインディングである `llama-cpp-2` をロードし、ローカルファイルシステム上の GGUF モデルを物理メモリに読み込んでコンテキストを構築します。

#### `LocalLlamaCppProvider::supports_vision`
*   **シグネチャ**: `pub fn supports_vision(&self) -> bool`
*   **説明**: マルチモーダル用のビジョンプロジェクションファイル（`mmproj`）がロードされ、視覚情報の解析がサポートされているかを返します。

#### `LocalLlamaCppProvider::summarize_rgb`
*   **シグネチャ**: `pub async fn summarize_rgb(&self, width: u32, height: u32, rgb: Vec<u8>, system: &str, user: &str) -> Result<String, LlmProviderError>`
*   **説明**: 生のスクリーンショット RGB フレームバッファとプロンプトテキストをローカルの視覚モデルに送信し、画面に表示されている内容の要約を生成します。

#### `LocalLlamaCppProvider::shutdown`
*   **シグネチャ**: `pub async fn shutdown(&self)`
*   **説明**: ロードされているモデルのコンテキスト、および C++ 側で確保されたシステムメモリリソースやスレッドプールを安全にクローズして開放します。

#### `validate_model_path`
*   **Signature**: `pub(crate) fn validate_model_path(path: &str) -> Result<PathBuf, LlmProviderError>`
*   **Description**: 指定された GGUF ファイルパスがシステム上に存在するか検証します。

#### `resolve_gpu_offload`
*   **Signature**: `pub(crate) fn resolve_gpu_offload(acceleration: ProactiveAcceleration, gpu_layers: &str) -> Result<GpuOffload, LlmProviderError>`
*   **Description**: システムの GPU 加速タイプとユーザー指定レイヤー設定に基づいて、GPU にオフロードすべきモデルテンソルレイヤー数を算出します。

#### `generate_chat`
*   **Signature**: `pub(crate) fn generate_chat(loaded: &LoadedModel, messages: &[LlmMessage], json_schema: Option<&serde_json::Value>, timeout: Duration) -> Result<String, LlmProviderError>`
*   **Description**: 会話履歴をテンプレートに展開して Llama.cpp コアに入力し、新規テキストトークンの生成処理を実行します。

#### `sample_tokens`
*   **Signature**: `fn sample_tokens(loaded: &LoadedModel, ctx: &mut llama_cpp_2::context::LlamaContext<'_>, batch: &mut LlamaBatch, json_schema: Option<&serde_json::Value>, deadline: Instant, max_tokens: i32, mut n_cur: i32) -> Result<String, LlmProviderError>`
*   **Description**: トークン評価ループにおいて、温度（Temperature）、Top-P サンプリング、および文法パーサー（Grammar）による出力フォーマット制限を適用しながらトークンを連続生成します。

#### `with_backend`
*   **Signature**: `pub(crate) fn with_backend<T, F>(f: F) -> Result<T, LlmProviderError> where F: FnOnce(&LlamaBackend) -> Result<T, LlmProviderError>`
*   **Description**: スレッドアンセーフな Llama.cpp バックエンドの複数スレッドからの同時アクセスを防ぐため、排他制御をかけたグローバルなロックラッピング関数です。

#### `embed_text`
*   **Signature**: `pub(crate) fn embed_text(loaded: &LoadedModel, text: &str) -> Result<Vec<f32>, LlmProviderError>`
*   **Description**: ローカルモデルのテキストベクトル埋め込み処理を呼び出します。

#### `create_local_provider`
*   **Signature**: `pub fn create_local_provider(local: &ResolvedLocalModel) -> Result<Box<dyn crate::EmbeddingProvider>, EneEmbeddingError>`
*   **Description**: ローカル GGUF 埋め込みモデル用のプロバイダ実体を初期化します。

---

## 4. モデルダウンロードとキャッシュ管理 (`gguf/`)

#### `ensure_gguf_available`
*   **シグネチャ**: `pub async fn ensure_gguf_available(local: &ResolvedLocalModel) -> Result<PathBuf, LlmProviderError>`
*   **説明**: 設定されたモデル名に対応する GGUF ファイルがローカルの `models` キャッシュフォルダに存在するか確認します。欠落している場合は非同期ダウンロードを開始します。

#### `ensure_mmproj_available`
*   **シグネチャ**: `pub async fn ensure_mmproj_available(local: &ResolvedLocalModel) -> Result<Option<PathBuf>, LlmProviderError>`
*   **説明**: 設定されたマルチモーダル用プロジェクタファイルがローカルに存在するか確認し、必要に応じて非同期ダウンロードを実行します。

#### `prefetch_configured_gguf`
*   **シグネチャ**: `pub async fn prefetch_configured_gguf(config: &AiConfig, prefetch_embedding: bool, prefetch_decision: bool) -> Result<(), LlmProviderError>`
*   **説明**: アプリケーションの起動時等に、構成設定をスキャンして必要となるすべての GGUF モデルファイルを事前バックグラウンド並列ダウンロードします。

#### `download_gguf`
*   **シグネチャ**: `pub async fn download_gguf(url: &str, dest: &Path) -> Result<(), LlmProviderError>`
*   **説明**: リダイレクト制御をサポートしたダウンロードタスクです。`.part` の一時拡張子ファイルを生成してデータをストリーミング受信し、ダウンロードが成功したのち正しいファイル名に変更して保存します。

#### `filename_from_url`
*   **Signature**: `pub fn filename_from_url(url: &str) -> Result<String, LlmProviderError>`
*   **Description**: モデルダウンロード URL から安全で有効なファイル名を抽出して返します。

---

## 5. RAG Retrieval 関連処理 (`hybrid.rs`)

#### `HybridRerankProvider::new`
*   **シグネチャ**: `pub fn new(embedder: Arc<dyn EmbeddingProvider>) -> Self`
*   **説明**: ハイブリッド再順位付け（Rerank）プロバイダを作成します。

#### `HybridRerankProvider::hyde`
*   **シグネチャ**: `pub async fn hyde(&self, query: &str) -> Result<String, EmbeddingError>`
*   **説明**: 仮説的回答作成モデル (HyDE) を使用して、ユーザーの短い質問から「予測回答テキスト」を仮構築し、ベクトル検索の一致効率を高めます。

#### `HybridRerankProvider::rerank`
*   **シグネチャ**: `pub async fn rerank(&self, query: &str, candidates: &[ene_plugin_proto::ToolSpec]) -> Result<Vec<f32>, EmbeddingError>`
*   **説明**: 抽出された候補について、交差注意（Cross-attention）モデル等の重み付けを使用してクエリとの関連度を再順位付け（Rerank）し、上位のみを残すフィルタを適用します。

#### `hyde_document`
*   **Signature**: `pub async fn hyde_document(llm: Option<&dyn LlmProvider>, query: &str) -> Result<String, EmbeddingError>`
*   **Description**: LLM プロバイダを用いて仮説的回答テキストの生成をリクエストします。

#### `rerank_tool_specs`
*   **Signature**: `pub async fn rerank_tool_specs(embedder: &dyn EmbeddingProvider, rerank_llm: Option<&dyn LlmProvider>, query: &str, candidates: &[ene_plugin_proto::ToolSpec]) -> Result<Vec<f32>, EmbeddingError>`
*   **Description**: 抽出されたツール定義仕様のクエリへの適合度を再計算してスコア付けします。
