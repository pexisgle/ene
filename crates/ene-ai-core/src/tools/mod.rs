pub mod builtin;
pub mod composite;
pub mod definition;
pub mod screenshot;
pub mod undo_manager;

pub mod delete;
pub mod edit;
pub mod patch;
pub mod question;
pub mod read;
pub mod registry;
pub mod search;
pub mod shell;
pub mod todo;
pub mod truncate;
pub mod undo_tool;
pub mod webfetch;
pub mod write;

// Cowork Agent ツール群 (Phase 2/3)
pub mod app;
pub mod browser;
pub mod websearch;

pub use builtin::BuiltinToolRegistry;
pub use composite::CompositeToolRegistry;
pub use definition::{ToolCallResult, ToolDefinition, ToolRegistry};
pub use registry::OpencodeToolRegistry;
pub use screenshot::ScreenshotToolRegistry;
pub use undo_manager::{UndoEntry, UndoManager, UndoOperation, backup_file};

/// ツールカテゴリ
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
