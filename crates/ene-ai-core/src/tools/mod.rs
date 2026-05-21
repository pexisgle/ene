pub mod composite;
pub mod definition;
pub mod utility;

pub use composite::CompositeToolRegistry;
pub use definition::{ToolCallResult, ToolDefinition, ToolRegistry};
pub use utility::undo_manager::{UndoEntry, UndoManager, UndoOperation, backup_file};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCategory {
    Filesystem,
    Shell,
    Browser,
    App,
    WebSearch,
    Utility,
}

impl ToolCategory {
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