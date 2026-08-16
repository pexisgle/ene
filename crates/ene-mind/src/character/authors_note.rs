#![expect(
    clippy::arithmetic_side_effects,
    reason = "mind pipeline uses intentional turn/score/index arithmetic"
)]
use ene_ai::Role;
use serde::{Deserialize, Serialize};

use crate::lifecycle::HistoryEntry;

/// Author's Note: a persistent instruction injected at a specific depth
/// in the conversation history.
///
/// Unlike the system prompt, this sits within the history at depth N
/// (e.g., 3–5 turns back from the end), keeping the main system prompt
/// clean while enforcing late-session behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorsNote {
    pub content: String,
    /// Insert at this depth from end of history (0 = most recent assistant turn).
    pub depth: usize,
    pub active: bool,
}

impl AuthorsNote {
    pub fn new(content: impl Into<String>, depth: usize) -> Self {
        Self {
            content: content.into(),
            depth,
            active: true,
        }
    }

    /// Uses `depth: 0` (most recent position) as a neutral default. When
    /// activated, the caller should set an appropriate depth (typically 3–5
    /// turns back from the end).
    pub fn inactive() -> Self {
        Self {
            content: String::new(),
            depth: 0,
            active: false,
        }
    }
}

impl Default for AuthorsNote {
    fn default() -> Self {
        Self::inactive()
    }
}

/// Apply an author's note to the history by inserting it as a user-role
/// message between history turns at the specified depth.
///
/// The note is inserted at `history.len() - depth - 1` positions from the end,
/// so a depth of 0 places it just before the most recent message, depth of 1
/// places it before the second-to-last, etc. If the depth exceeds the history
/// length, the note is inserted at the beginning of history.
///
/// The note uses `Role::User` (not `Role::System`) because some providers
/// (e.g., Anthropic) reject system messages in the middle of history. The
/// `[Author's Note]` prefix makes the instructional intent clear.
pub fn apply_authors_note(history: &mut Vec<HistoryEntry>, note: &AuthorsNote) {
    let Some(content) = render_authors_note(note) else {
        return;
    };

    let insert_at = if history.len() > note.depth + 1 {
        history.len() - note.depth - 1
    } else {
        0
    };

    let note_entry = HistoryEntry {
        role: Role::User,
        content,
    };

    history.insert(insert_at, note_entry);
}

/// The exact history-entry content [`apply_authors_note`] would insert, or
/// `None` when the note is inactive or blank and nothing is inserted.
///
/// Prompt packing must reserve the note's tokens before filling the budget, but
/// the note itself is inserted only after history trimming (so its depth is
/// relative to the post-trim history). Both steps therefore need the same
/// rendered text; sharing this function keeps the reservation exact instead of
/// re-deriving the prefix and risking drift.
#[must_use]
pub fn render_authors_note(note: &AuthorsNote) -> Option<String> {
    if !note.active || note.content.trim().is_empty() {
        return None;
    }
    Some(format!("[Author's Note]\n{}", note.content.trim()))
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::indexing_slicing,
        reason = "tests index into fixed-size fixture vectors"
    )]
    use super::*;

    #[test]
    fn inactive_note_not_applied() {
        let note = AuthorsNote::inactive();
        let mut history = vec![HistoryEntry {
            role: Role::User,
            content: "hello".into(),
        }];
        apply_authors_note(&mut history, &note);
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn empty_note_not_applied() {
        let note = AuthorsNote::new("", 3);
        let mut history = vec![HistoryEntry {
            role: Role::User,
            content: "hello".into(),
        }];
        apply_authors_note(&mut history, &note);
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn note_inserted_at_correct_depth() {
        let note = AuthorsNote::new("Stay in character.", 1);
        let mut history = vec![
            HistoryEntry {
                role: Role::User,
                content: "msg1".into(),
            },
            HistoryEntry {
                role: Role::Assistant,
                content: "reply1".into(),
            },
            HistoryEntry {
                role: Role::User,
                content: "msg2".into(),
            },
            HistoryEntry {
                role: Role::Assistant,
                content: "reply2".into(),
            },
        ];
        apply_authors_note(&mut history, &note);
        assert_eq!(history.len(), 5);
        assert_eq!(history[2].role, Role::User);
        assert!(history[2].content.contains("Stay in character."));
        assert_eq!(history[3].content, "msg2");
        assert_eq!(history[4].content, "reply2");
    }

    #[test]
    fn note_depth_exceeds_history_length() {
        let note = AuthorsNote::new("Important note.", 100);
        let mut history = vec![
            HistoryEntry {
                role: Role::User,
                content: "hello".into(),
            },
            HistoryEntry {
                role: Role::Assistant,
                content: "hi".into(),
            },
        ];
        apply_authors_note(&mut history, &note);
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].role, Role::User);
        assert!(history[0].content.contains("Important note."));
        assert_eq!(history[1].content, "hello");
        assert_eq!(history[2].content, "hi");
    }
}
