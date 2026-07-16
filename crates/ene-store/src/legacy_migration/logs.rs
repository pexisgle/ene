//! Convert legacy conversation logs into memory spans.

use crate::store::NewMemorySpan;

/// A single log row used when building spans.
#[derive(Debug, Clone)]
pub struct LegacyLogRow {
    /// Session identifier.
    pub session_id: String,
    /// Message role (`user` or `assistant`).
    pub role: String,
    /// Message content.
    pub content: String,
}

/// Group consecutive user/assistant log rows into memory spans.
#[must_use]
pub fn logs_to_spans(rows: &[LegacyLogRow]) -> Vec<NewMemorySpan> {
    let mut spans = Vec::new();
    let mut turn_index: i32 = 0;
    let mut i = 0usize;

    while i < rows.len() {
        let Some(row) = rows.get(i) else {
            break;
        };
        let session_id = row.session_id.clone();
        let turn_start = turn_index;
        let mut excerpt_parts = Vec::new();

        if row.role == "user" {
            excerpt_parts.push(format!("User: {}", row.content));
            i = i.saturating_add(1);
            if let Some(assistant) = rows.get(i)
                && assistant.session_id == session_id
                && assistant.role == "assistant"
            {
                excerpt_parts.push(format!("Assistant: {}", assistant.content));
                i = i.saturating_add(1);
            }
        } else {
            excerpt_parts.push(format!("{}: {}", row.role, row.content));
            i = i.saturating_add(1);
        }

        let turn_end = turn_index;
        let raw_excerpt = excerpt_parts.join("\n");
        spans.push(NewMemorySpan {
            session_id,
            turn_start,
            turn_end,
            raw_excerpt: Some(raw_excerpt),
            compressed_summary: None,
            compression_level: 0,
        });
        turn_index = turn_index.saturating_add(1);
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_user_assistant() {
        let rows = vec![
            LegacyLogRow {
                session_id: "s1".to_string(),
                role: "user".to_string(),
                content: "hi".to_string(),
            },
            LegacyLogRow {
                session_id: "s1".to_string(),
                role: "assistant".to_string(),
                content: "hello".to_string(),
            },
        ];
        let spans = logs_to_spans(&rows);
        assert_eq!(spans.len(), 1);
        let span = spans.first().expect("one span");
        assert!(span.raw_excerpt.as_ref().unwrap().contains("hi"));
        assert!(span.raw_excerpt.as_ref().unwrap().contains("hello"));
    }
}
