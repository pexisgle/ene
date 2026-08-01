//! Actor-native undo stack and metadata.
//!
//! The runtime records one [`UndoEntry`] per mutating tool call so the
//! `/undo` command can surface what a rollback affects and, for reversible
//! operations, drive the owning tool's undo action. Reversibility is
//! classified from the tool name: filesystem mutations are reversible via
//! the fs tool's DB-backed undo manager, while shell execution and other
//! side-effecting tools are treated as irreversible.

use ene_plugin_proto::{Reversibility, UndoMetadata};

/// Tools whose mutations are reversible through the fs undo manager.
const REVERSIBLE_TOOLS: &[&str] = &[
    "filesystem.write",
    "filesystem.edit",
    "filesystem.patch",
    "filesystem.delete",
];

/// Tools that are inherently irreversible (no safe rollback).
const IRREVERSIBLE_TOOLS: &[&str] = &["shell.execute"];

/// Classify a tool call's reversibility from its namespaced name.
///
/// Filesystem mutating tools are reversible; shell execution and unknown
/// mutating tools default to irreversible (fail-safe: we never claim an
/// operation is undoable unless we know how to undo it).
#[must_use]
pub fn classify_reversibility(tool_name: &str) -> Reversibility {
    if REVERSIBLE_TOOLS.contains(&tool_name) {
        Reversibility::Reversible
    } else {
        Reversibility::Irreversible
    }
}

/// Whether a tool call is a mutation worth recording on the undo stack.
///
/// Only reversible filesystem mutations are recorded. Read-only tools,
/// meta tools, and irreversible tools (shell execution, external sends)
/// are excluded — irreversible actions are warned about at execution time
/// but never placed on the undo stack.
#[must_use]
pub fn is_undo_relevant(tool_name: &str) -> bool {
    REVERSIBLE_TOOLS.contains(&tool_name)
}

/// Whether a tool call is irreversible (cannot be rolled back).
///
/// Used to emit a warning before execution so the user knows the
/// operation cannot be undone.
#[must_use]
pub fn is_irreversible(tool_name: &str) -> bool {
    IRREVERSIBLE_TOOLS.contains(&tool_name)
}

/// A single recorded undo checkpoint on the actor's stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoEntry {
    /// The undo metadata for the recorded operation.
    pub metadata: UndoMetadata,
}

/// Outcome of an `/undo` request, reported back to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoReport {
    /// No reversible operation is available to undo.
    NothingToUndo,
    /// The most recent operation is irreversible and cannot be rolled back.
    Irreversible {
        /// Metadata for the irreversible operation.
        metadata: UndoMetadata,
    },
    /// A reversible operation was successfully reverted.
    Reverted {
        /// Metadata for the reverted operation.
        metadata: UndoMetadata,
        /// The owning tool's undo output.
        output: String,
    },
    /// A reversible operation was found but the rollback failed.
    Failed {
        /// Metadata for the operation that failed to revert.
        metadata: UndoMetadata,
        /// The error reported by the owning tool.
        error: String,
    },
}

/// Best-effort extraction of target resources from a tool call's JSON args.
///
/// Looks for common path/command fields (`path`, `file_path`, `command`,
/// `target`) so the undo report can show what an operation affected.
#[must_use]
pub fn extract_targets(arguments: &str) -> Vec<String> {
    const KEYS: &[&str] = &["path", "file_path", "command", "target"];
    let Ok(value) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return Vec::new();
    };
    let Some(obj) = value.as_object() else {
        return Vec::new();
    };
    KEYS.iter()
        .filter_map(|key| obj.get(*key))
        .filter_map(serde_json::Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Bounded LIFO stack of undo checkpoints, newest last.
#[derive(Debug, Default)]
pub struct UndoStack {
    entries: Vec<UndoEntry>,
    /// Hard cap so a long-running session cannot grow the stack unbounded.
    capacity: usize,
}

impl UndoStack {
    /// Create a stack with the given capacity (oldest entries evicted).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::new(),
            capacity: capacity.max(1),
        }
    }

    /// Record a mutating tool call. Irrelevant tools are ignored.
    pub fn record(&mut self, tool_name: &str, turn_id: &str, target_resources: Vec<String>) {
        if !is_undo_relevant(tool_name) {
            return;
        }
        let metadata = UndoMetadata {
            turn_id: turn_id.to_string(),
            tool_name: tool_name.to_string(),
            target_resources,
            reversibility: classify_reversibility(tool_name),
        };
        self.entries.push(UndoEntry { metadata });
        if self.entries.len() > self.capacity {
            let overflow = self.entries.len() - self.capacity;
            self.entries.drain(0..overflow);
        }
    }

    /// The most recent reversible entry, if any (does not remove it).
    #[must_use]
    pub fn peek_reversible(&self) -> Option<&UndoEntry> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.metadata.reversibility == Reversibility::Reversible)
    }

    /// The most recent entry regardless of reversibility (does not remove it).
    #[must_use]
    pub fn peek(&self) -> Option<&UndoEntry> {
        self.entries.last()
    }

    /// Remove and return the most recent reversible entry.
    pub fn pop_reversible(&mut self) -> Option<UndoEntry> {
        let pos = self
            .entries
            .iter()
            .rposition(|e| e.metadata.reversibility == Reversibility::Reversible)?;
        Some(self.entries.remove(pos))
    }

    /// All entries, oldest first.
    #[must_use]
    pub fn entries(&self) -> &[UndoEntry] {
        &self.entries
    }

    /// Number of recorded entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the stack is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_reversible_and_irreversible() {
        assert_eq!(
            classify_reversibility("filesystem.write"),
            Reversibility::Reversible
        );
        assert_eq!(
            classify_reversibility("shell.execute"),
            Reversibility::Irreversible
        );
        assert_eq!(
            classify_reversibility("unknown.tool"),
            Reversibility::Irreversible
        );
    }

    #[test]
    fn ignores_irrelevant_tools() {
        let mut stack = UndoStack::new(10);
        stack.record("filesystem.read", "t1", vec!["/a".into()]);
        stack.record("system.search_tools", "t1", vec![]);
        // Irreversible tools are warned about but never recorded.
        stack.record("shell.execute", "t1", vec!["ls".into()]);
        assert!(stack.is_empty());
    }

    #[test]
    fn records_and_pops_reversible() {
        let mut stack = UndoStack::new(10);
        stack.record("filesystem.write", "t1", vec!["/a".into()]);
        stack.record("filesystem.edit", "t2", vec!["/b".into()]);
        assert_eq!(stack.len(), 2);
        assert_eq!(
            stack
                .peek_reversible()
                .map(|e| e.metadata.tool_name.as_str()),
            Some("filesystem.edit")
        );
        let popped = stack.pop_reversible().expect("reversible entry");
        assert_eq!(popped.metadata.tool_name, "filesystem.edit");
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn irreversible_classification() {
        assert!(is_irreversible("shell.execute"));
        assert!(!is_irreversible("filesystem.write"));
    }

    #[test]
    fn evicts_oldest_over_capacity() {
        let mut stack = UndoStack::new(2);
        stack.record("filesystem.write", "t1", vec!["/a".into()]);
        stack.record("filesystem.edit", "t2", vec!["/b".into()]);
        stack.record("filesystem.delete", "t3", vec!["/c".into()]);
        assert_eq!(stack.len(), 2);
        assert_eq!(stack.entries()[0].metadata.turn_id, "t2");
    }

    #[test]
    fn extracts_target_paths() {
        let args = r#"{"path":"/tmp/x","content":"hi"}"#;
        assert_eq!(extract_targets(args), vec!["/tmp/x".to_string()]);
        assert!(extract_targets("not json").is_empty());
        assert!(extract_targets("{}").is_empty());
    }
}
