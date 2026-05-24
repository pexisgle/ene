use async_trait::async_trait;

use super::ToolDefinition;

/// ツールレジストリ — 組み込みツールやMCPツールの統一インターフェース
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    /// 利用可能なツール一覧を返す
    fn list_tools(&self) -> Vec<ToolDefinition>;

    /// クエリに関連するツールのみを返す（デフォルトでは全ツール）
    fn list_relevant_tools(
        &self,
        _query_embedding: Option<&[f32]>,
        _limit: usize,
    ) -> Vec<ToolDefinition> {
        self.list_tools()
    }

    /// ツールを実行する
    async fn call_tool(
        &self,
        name: &str,
        arguments: &str, // JSON string from LLM
    ) -> Result<String, crate::error::ToolError>;

    /// 現在のセッションIDを設定（Undo等で使用）
    async fn set_session_id(&self, _session_id: &str) {}

    /// 破壊的操作の承認（リクエストID）
    async fn approve_permission(&self, _request_id: &str) {}

    /// セッション全体のパーミッション許可パターンの追加
    async fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// RAGインデックスが必要な場合に構築する（デフォルトでは何もしない）
    async fn ensure_index_built(
        &self,
        _embedder: &dyn ene_embedding::EmbeddingProvider,
        _store: Option<&ene_memory::MemoryStore>,
    ) -> Result<(), crate::error::ToolError> {
        Ok(())
    }
}
