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

pub use definition::{ToolDefinition, ToolRegistry, ToolCallResult};
pub use builtin::BuiltinToolRegistry;
pub use screenshot::ScreenshotToolRegistry;
pub use composite::CompositeToolRegistry;
pub use undo_manager::{UndoManager, UndoEntry, UndoOperation, backup_file};
pub use registry::OpencodeToolRegistry;
