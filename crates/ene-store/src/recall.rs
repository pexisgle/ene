//! Formats past conversation summaries for prompt injection.
//!
//! All display strings are sourced from the [`PromptLibrary`] so they can be
//! localised without recompiling.

use super::store::RecalledSummary;
use chrono::Utc;
use ene_common::truncate::Truncate;
use ene_config::PromptLibrary;

/// Formats past conversation summaries into a text block for prompt injection.
///
/// Uses the prompt library for the section header and item format so the
/// strings stay out of compiled code and can be localised by swapping the
/// locale file.
#[must_use]
pub fn format_summaries_for_prompt(summaries: &[RecalledSummary]) -> String {
    if summaries.is_empty() {
        return String::new();
    }
    let prompts = PromptLibrary::load("en");
    format_summaries_with_library(summaries, &prompts)
}

/// Same as [`format_summaries_for_prompt`] but accepts a pre-loaded library,
/// avoiding a redundant file-system read when the caller already holds one.
#[must_use]
pub fn format_summaries_with_library(
    summaries: &[RecalledSummary],
    prompts: &PromptLibrary,
) -> String {
    if summaries.is_empty() {
        return String::new();
    }

    let now = Utc::now();
    let header = &prompts.memory().summaries_header;
    let item_tpl = &prompts.memory().summary_item;

    let mut lines = vec![header.to_string()];
    for s in summaries {
        let age = format_age(now - s.entry.ended_at);
        let text = Truncate::simple(&s.entry.summary, 300);
        let line = if item_tpl.is_empty() {
            format!("[{age}] {text}")
        } else {
            prompts.memory().render_summary_item(&age, &text)
        };
        lines.push(line);
    }

    lines.join("\n")
}

fn format_age(dur: chrono::Duration) -> String {
    let total_seconds = dur.num_seconds().max(0);
    if total_seconds < 60 {
        "just now".to_string()
    } else if total_seconds < 3600 {
        let mins = total_seconds / 60;
        if mins == 1 {
            "1 min ago".to_string()
        } else {
            format!("{mins} min ago")
        }
    } else if total_seconds < 86400 {
        let hours = total_seconds / 3600;
        if hours == 1 {
            "1 hour ago".to_string()
        } else {
            format!("{hours} hours ago")
        }
    } else {
        let days = total_seconds / 86400;
        if days == 1 {
            "1 day ago".to_string()
        } else {
            format!("{days} days ago")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_format_age() {
        assert_eq!(format_age(Duration::seconds(30)), "just now");
        assert_eq!(format_age(Duration::minutes(5)), "5 min ago");
        assert_eq!(format_age(Duration::hours(3)), "3 hours ago");
        assert_eq!(format_age(Duration::days(2)), "2 days ago");
    }

    #[test]
    fn format_summaries_for_prompt_empty_returns_empty() {
        assert!(format_summaries_for_prompt(&[]).is_empty());
    }
}
