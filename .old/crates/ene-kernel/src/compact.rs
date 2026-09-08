//! Extractive compaction: keep topic + recent points, not a prefix chop.

use ene_session::{ProjectedMessage, Role};

pub(crate) const MAX_SUMMARY_CHARS: usize = 2_000;

#[must_use]
pub(crate) fn summarize_history(messages: &[ProjectedMessage], max_chars: usize) -> String {
    let lines: Vec<String> = messages.iter().filter_map(format_line).collect();
    stitch_head_and_tail(&lines, max_chars.max(1))
}

fn format_line(message: &ProjectedMessage) -> Option<String> {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        Role::Thinking | Role::Inner | Role::Tool | Role::Status => return None,
    };
    let text = message.text();
    if text.trim().is_empty() {
        return None;
    }
    Some(format!("- {role}: {text}"))
}

fn stitch_head_and_tail(lines: &[String], max_chars: usize) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let joined = lines.join("\n");
    if joined.chars().count() <= max_chars {
        return joined;
    }
    let head_budget = (max_chars / 4).max(24);
    let head = shorten_line(&lines[0], head_budget);
    let mut used = head.chars().count();
    let mut tail = Vec::new();
    for line in lines.iter().skip(1).rev() {
        let remaining = max_chars.saturating_sub(used.saturating_add(1));
        if remaining < 8 {
            break;
        }
        let piece = shorten_line(line, remaining);
        let add = piece.chars().count().saturating_add(1);
        if used.saturating_add(add) > max_chars {
            break;
        }
        used = used.saturating_add(add);
        tail.push(piece);
    }
    tail.reverse();
    let mut out = vec![head];
    out.extend(tail);
    out.join("\n")
}

fn shorten_line(line: &str, budget: usize) -> String {
    if line.chars().count() <= budget {
        return line.to_owned();
    }
    take_chars(line, budget)
}

fn take_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::{MAX_SUMMARY_CHARS, summarize_history};
    use ene_session::{Block, ProjectedMessage, Role};

    fn msg(role: Role, text: &str) -> ProjectedMessage {
        ProjectedMessage {
            seq: 1,
            role,
            blocks: vec![Block::text(text)],
            turn_id: None,
            step_index: None,
            tool_name: None,
            tool_args: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn under_budget_keeps_every_line() {
        let summary = summarize_history(
            &[msg(Role::User, "hello"), msg(Role::Assistant, "hi there")],
            MAX_SUMMARY_CHARS,
        );
        assert!(summary.contains("hello"));
        assert!(summary.contains("hi there"));
    }

    #[test]
    fn over_budget_keeps_topic_and_latest_point() {
        let topic = format!("{} start-topic", "alpha ".repeat(400));
        let summary = summarize_history(
            &[
                msg(Role::User, &topic),
                msg(Role::Assistant, "ack"),
                msg(Role::User, "later-point-omega"),
            ],
            180,
        );
        assert!(
            summary.contains("later-point-omega"),
            "recent point dropped: {summary}"
        );
        assert!(
            summary.contains("start-topic") || summary.contains("alpha"),
            "topic dropped: {summary}"
        );
        assert!(summary.chars().count() <= 180);
        assert!(
            !summary.starts_with(&topic.chars().take(180).collect::<String>())
                || summary.contains("later-point-omega"),
            "looks like a prefix chop: {summary}"
        );
    }

    #[test]
    fn inner_and_tool_lines_are_omitted() {
        let summary = summarize_history(
            &[
                msg(Role::User, "visible"),
                msg(Role::Inner, "hidden inner body"),
                msg(Role::Tool, "fs.read"),
            ],
            MAX_SUMMARY_CHARS,
        );
        assert!(summary.contains("visible"));
        assert!(!summary.contains("hidden inner body"));
        assert!(!summary.contains("fs.read"));
    }

    #[test]
    fn under_budget_keeps_indented_content() {
        let summary = summarize_history(
            &[msg(Role::Assistant, "def foo():\n    return 1\n")],
            MAX_SUMMARY_CHARS,
        );
        assert!(summary.contains("    return 1"));
    }

    #[test]
    fn over_budget_keeps_past_a_short_opener() {
        let body = format!("Sure. {}", "results ".repeat(40));
        let summary = summarize_history(&[msg(Role::Assistant, &body)], 80);
        assert!(
            summary.contains("Sure.") && summary.contains("resul"),
            "short opener ate the budget: {summary}"
        );
        assert!(summary.chars().count() <= 80);
    }
}
