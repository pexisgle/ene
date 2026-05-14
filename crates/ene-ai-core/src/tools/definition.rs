use async_trait::async_trait;

/// ツール定義 — OpenAI API の `tools` パラメータに渡す形式
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value, // JSON Schema
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

    /// ツールを実行する
    async fn call_tool(
        &self,
        name: &str,
        arguments: &str, // JSON string from LLM
    ) -> Result<String, String>;

    /// 現在のセッションIDを設定（Undo等で使用）
    fn set_session_id(&self, _session_id: &str) {}
}
