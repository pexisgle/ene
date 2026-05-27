use ene_config::serde::{Deserialize, Serialize};

/// Tool category — used for classification and RAG filtering
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(crate = "ene_config::serde")]
pub enum ToolCategory {
    /// File-system tools (read, write, edit, shell, undo).
    Filesystem,
    /// Shell execution tools.
    Shell,
    /// Browser automation tools.
    Browser,
    /// GUI automation tools.
    App,
    /// Web search tools.
    WebSearch,
    /// Utility tools (question, todo, time, system info).
    Utility,
}

impl ToolCategory {
    /// Returns the category label used for RAG embedding.
    pub fn label(&self) -> &'static str {
        match self {
            ToolCategory::Filesystem => "filesystem_tools",
            ToolCategory::Shell => "shell_tools",
            ToolCategory::Browser => "browser_tools",
            ToolCategory::App => "app_tools",
            ToolCategory::WebSearch => "websearch_tools",
            ToolCategory::Utility => "utility_tools",
        }
    }
}

/// Tool definition — format passed to the OpenAI API `tools` parameter
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(crate = "ene_config::serde")]
pub struct ToolDefinition {
    /// Unique tool name.
    pub name: String,
    /// Human-readable description of what this tool does.
    pub description: String,
    /// JSON Schema describing the tool's parameters.
    pub parameters: serde_json::Value,
    /// Optional category for RAG filtering.
    pub category: Option<ToolCategory>,
    /// Keywords for RAG-based tool retrieval.
    pub keywords: Vec<String>,
}

impl ToolDefinition {
    /// Generates embedding text for RAG search
    pub fn embedding_text(&self) -> String {
        let keywords = if self.keywords.is_empty() {
            String::new()
        } else {
            format!(" Keywords: {}", self.keywords.join(", "))
        };
        let category = self
            .category
            .map(|c| format!(" Category: {}", c.label()))
            .unwrap_or_default();
        format!(
            "{}: {}{}{}",
            self.name, self.description, category, keywords
        )
    }
}

/// Tool execution result (format returned to the LLM)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(crate = "ene_config::serde")]
pub struct ToolCallResult {
    /// Identifier matching the original tool call.
    pub tool_call_id: String,
    /// The tool's output content.
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_category_label() {
        assert_eq!(ToolCategory::Filesystem.label(), "filesystem_tools");
        assert_eq!(ToolCategory::Shell.label(), "shell_tools");
        assert_eq!(ToolCategory::Browser.label(), "browser_tools");
        assert_eq!(ToolCategory::App.label(), "app_tools");
        assert_eq!(ToolCategory::WebSearch.label(), "websearch_tools");
        assert_eq!(ToolCategory::Utility.label(), "utility_tools");
    }

    #[test]
    fn tool_category_serde_roundtrip() {
        for cat in &[
            ToolCategory::Filesystem,
            ToolCategory::Shell,
            ToolCategory::Browser,
            ToolCategory::App,
            ToolCategory::WebSearch,
            ToolCategory::Utility,
        ] {
            let json = serde_json::to_string(cat).unwrap();
            let deser: ToolCategory = serde_json::from_str(&json).unwrap();
            assert_eq!(*cat, deser);
        }
    }

    #[test]
    fn tool_definition_embedding_text_no_keywords() {
        let def = ToolDefinition {
            name: "test_tool".into(),
            description: "A test tool".into(),
            parameters: serde_json::json!({}),
            category: Some(ToolCategory::Utility),
            keywords: vec![],
        };
        let text = def.embedding_text();
        assert_eq!(text, "test_tool: A test tool Category: utility_tools");
    }

    #[test]
    fn tool_definition_embedding_text_with_keywords() {
        let def = ToolDefinition {
            name: "my_tool".into(),
            description: "My custom tool".into(),
            parameters: serde_json::json!({"type": "object"}),
            category: None,
            keywords: vec!["foo".into(), "bar".into()],
        };
        let text = def.embedding_text();
        assert_eq!(text, "my_tool: My custom tool Keywords: foo, bar");
    }

    #[test]
    fn tool_definition_serde_roundtrip() {
        let def = ToolDefinition {
            name: "test".into(),
            description: "desc".into(),
            parameters: serde_json::json!({"type": "object"}),
            category: Some(ToolCategory::Shell),
            keywords: vec!["kw".into()],
        };
        let json = serde_json::to_string(&def).unwrap();
        let deser: ToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(def, deser);
    }

    #[test]
    fn tool_call_result_serde_roundtrip() {
        let result = ToolCallResult {
            tool_call_id: "call_123".into(),
            content: "result content".into(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let deser: ToolCallResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, deser);
    }

}
