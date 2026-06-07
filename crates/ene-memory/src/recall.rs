use super::store::RecalledSummary;
use chrono::Utc;
use ene_common::truncate::Truncate;

/// Formats past conversation summaries into a text block for prompt injection
#[must_use]
pub fn format_summaries_for_prompt(summaries: &[RecalledSummary]) -> String {
    if summaries.is_empty() {
        return String::new();
    }

    let now = Utc::now();
    let mut lines =
        vec!["[Past Conversation Summaries — relevant previous conversations]".to_string()];

    for s in summaries {
        let age = format_age(now - s.entry.ended_at);
        lines.push(format!(
            "- ({}) Summary: {}",
            age,
            Truncate::simple(&s.entry.summary, 300)
        ));
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
            "1 minute ago".to_string()
        } else {
            format!("{mins} minutes ago")
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
        assert_eq!(format_age(Duration::minutes(5)), "5 minutes ago");
        assert_eq!(format_age(Duration::hours(3)), "3 hours ago");
        assert_eq!(format_age(Duration::days(2)), "2 days ago");
    }
}
