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
    ) -> Result<String, String>;

    /// 現在のセッションIDを設定（Undo等で使用）
    async fn set_session_id(&self, _session_id: &str) {}

    /// RAGインデックスが必要な場合に構築する（デフォルトでは何もしない）
    async fn ensure_index_built(
        &self,
        _embedder: &dyn crate::embedding::EmbeddingProvider,
        _store: Option<&crate::memory::store::MemoryStore>,
    ) -> Result<(), String> {
        Ok(())
    }
}
