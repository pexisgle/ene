pub mod definition;
pub mod builtin;
pub mod screenshot;
pub mod composite;
pub mod undo_manager;

pub mod truncate;
pub mod read;
pub mod write;
pub mod edit;
pub mod search;
pub mod shell;
pub mod delete;
pub mod undo_tool;
pub mod patch;
pub mod todo;
pub mod webfetch;
pub mod question;
pub mod registry;

// Cowork Agent ツール群 (Phase 2/3)
pub mod browser;
pub mod app;
pub mod websearch;

pub use definition::{ToolDefinition, ToolRegistry, ToolCallResult};
pub use builtin::BuiltinToolRegistry;
pub use screenshot::ScreenshotToolRegistry;
pub use composite::CompositeToolRegistry;
pub use undo_manager::{UndoManager, UndoEntry, UndoOperation, backup_file};
pub use registry::OpencodeToolRegistry;

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
