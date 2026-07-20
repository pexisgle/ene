# `ene-ai` AIプロバイダ & 量子化GGUF推論仕様

`ene-ai` クレートは、LLM（Large Language Model）およびテキスト埋め込み（Embedding）プロバイダーの共通インターフェース（トレイト）の定義と、OpenAI互換クラウドサービスおよびローカルに配置された GGUF 量子化ファイルのインプロセス推論エンジンを提供します。

---

## 1. 共通インターフェース (Traits)

### `LlmProvider`
*   **シグネチャ**:
    ```rust
    #[async_trait]
    pub trait LlmProvider: Send + Sync {
        async fn stream_chat(
            &self,
            messages: &[LlmMessage],
            tools: &[ToolSpec],
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>, LlmProviderError>;
        fn model_name(&self) -> &str;
    }
    ```
*   **解説**: メッセージ履歴と利用可能ツール定義を受け取り、非同期にストリーミングトークン（`LlmResponseChunk`）を生成するチャネルストリームを返却します。

### `EmbeddingProvider`
*   **シグネチャ**:
    ```rust
    #[async_trait]
    pub trait EmbeddingProvider: Send + Sync {
        async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError>;
        fn model_name(&self) -> &str;
    }
    ```
*   **解説**: 与えられた文字列スライスのバッチ（複数テキスト）に対して、一度に埋め込みベクトル（浮動小数点配列）を生成します。

---

## 2. クラウド & ローカルプロバイダー実装

### 1. `OpenAiProvider` (クラウド実装)
*   **技術詳細**: `async-openai` クレートをラップし、OpenAI 互換エンドポイントに対して HTTPS リクエストを送信します。
*   **エラー処理**: 接続遮断やレートリミットを検知した場合、`LlmProviderError::ApiError` にマッピングして上位に返します。

### 2. `LocalLlamaCppProvider` (ローカル推論)
*   **技術詳細**: `llama-cpp-2` クレートを介して、インプロセスで C++ 側コア（`llama.cpp`）を呼び出し、モデルのロード、コンテキスト初期化、およびトークンサンプリングを実行します。
*   **アテンション/並行処理**: ローカルハードウェアアクセラレーション（CUDA/Metal）がある場合、Nix Nix-shell / Flake を介して共有ライブラリが適切にロードされ、スレッド並行処理されます。

---

## 3. GGUFモデル管理と自動ダウンロード (`gguf.rs`)

ローカル推論で使用される GGUF ファイル（埋め込み用の `nomic-embed` や、能動発話判定用の軽量Llama等）をダウンロード・配置するためのヘルパー。

*   **配置ディレクトリ**: `ene_config::paths::models_dir()` (通常は `~/.gemini/antigravity/models/` 等)。
*   **`ensure_gguf_available`**:
    指定されたURLとファイル名に基づき、すでにローカルに存在するかチェックし、無ければ `reqwest` で非同期ダウンロードを実行します（プログレスログを subscriber へ送信）。
*   **`prefetch_configured_gguf`**:
    設定ファイル `AiConfig` を走査し、ローカル（GGUF）で指定されている埋め込みモデル、および意思決定モデルのダウンロードを並列で実行します（起動時の初期化シーケンスで呼び出されます）。
*   ** vision/mmproj モデル**:
    スクリーンショット画像解釈用の `mmproj` ファイルのダウンロード・検証フックも提供します。
