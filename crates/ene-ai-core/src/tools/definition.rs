use async_trait::async_trait;
use super::ToolCategory;

/// ツール定義 — OpenAI API の `tools` パラメータに渡す形式
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value, // JSON Schema
    pub category: Option<ToolCategory>,
    pub keywords: Vec<String>,
}

impl ToolDefinition {
    /// RAG検索用のembeddingテキストを生成する
    pub fn embedding_text(&self) -> String {
        let keywords = if self.keywords.is_empty() {
            String::new()
        } else {
            format!(" Keywords: {}", self.keywords.join(", "))
        };
        let category = self.category.map(|c| format!(" Category: {}", c.label())).unwrap_or_default();
        format!("{}: {}{}{}", self.name, self.description, category, keywords)
    }
}

/// ツール実行結果
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub tool_call_id: String,
    pub content: String,
}

/// ツールレジストリ — 組み込みツールやMCPツールの統一インターフェース
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    /// 利用可能なツール一覧を返す
    fn list_tools(&self) -> Vec<ToolDefinition>;

    /// クエリに関連するツールのみを返す（デフォルトでは全ツール）
    fn list_relevant_tools(&self, _query_embedding: Option<&[f32]>, _limit: usize) -> Vec<ToolDefinition> {
        self.list_tools()
    }

    /// ツールを実行する
    async fn call_tool(
        &self,
        name: &str,
        arguments: &str, // JSON string from LLM
    ) -> Result<String, String>;

    /// 現在のセッションIDを設定（Undo等で使用）
    fn set_session_id(&self, _session_id: &str) {}

    /// RAGインデックスが必要な場合に構築する（デフォルトでは何もしない）
    async fn ensure_index_built(
        &self,
        _embedder: &dyn crate::embedding::EmbeddingProvider,
        _store: Option<&crate::memory::store::MemoryStore>,
    ) -> Result<(), String> {
        Ok(())
    }
}
