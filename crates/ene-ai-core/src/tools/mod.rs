pub mod app;
pub mod browser;
pub mod composite;
pub mod core;
pub mod definition;
pub mod delete;
pub mod edit;
pub mod filesystem;
pub mod patch;
pub mod read;
pub mod search;
pub mod shell;
pub mod utility;
pub mod web;
pub mod write;

pub use composite::CompositeToolRegistry;
pub use core::EneToolRegistry;
pub use definition::{ToolCallResult, ToolDefinition, ToolRegistry};
pub use utility::builtin::BuiltinToolRegistry;
pub use utility::screenshot::ScreenshotToolRegistry;
pub use utility::undo_manager::{UndoEntry, UndoManager, UndoOperation, backup_file};

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
