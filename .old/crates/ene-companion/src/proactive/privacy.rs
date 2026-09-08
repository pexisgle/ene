use crate::config::ObservationTitleMode;

/// Privacy-safe window metadata for the model and proactive context.
#[must_use]
pub fn redact_window_title(label: &str, mode: ObservationTitleMode) -> String {
    match mode {
        ObservationTitleMode::AppOnly => app_name_only(label),
        ObservationTitleMode::RedactedTitle => redact_tokens(label),
        ObservationTitleMode::FullTitle => label.trim().to_owned(),
    }
}

fn app_name_only(label: &str) -> String {
    let trimmed = label.trim();
    for sep in [" — ", " – ", " － ", " - ", " | "] {
        if let Some((_, app)) = trimmed.rsplit_once(sep) {
            let app = app.trim();
            if !app.is_empty() && !is_sensitive_token(app) {
                return app.to_owned();
            }
        }
    }
    String::new()
}

fn redact_tokens(label: &str) -> String {
    let mut out = String::new();
    for token in label.split_whitespace() {
        if is_sensitive_token(token) {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(token);
    }
    if out.is_empty() {
        "window".to_owned()
    } else {
        out
    }
}

fn is_sensitive_token(token: &str) -> bool {
    token.contains('@') || is_path_like(token) || is_url_like(token) || is_digit_heavy(token)
}

fn is_path_like(token: &str) -> bool {
    token.contains('/')
        || token.contains('\\')
        || token.contains('／')
        || token.starts_with('~')
        || drive_prefix(token)
}

fn drive_prefix(token: &str) -> bool {
    let mut chars = token.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic()
    )
}

fn is_url_like(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    lower.contains("://")
        || lower.starts_with("www.")
        || lower.contains(".jp/")
        || lower.contains(".com/")
        || lower.contains(".net/")
        || lower.contains(".org/")
}

fn is_digit_heavy(token: &str) -> bool {
    let digits = token.chars().filter(char::is_ascii_digit).count();
    let chars = token.chars().count();
    digits >= 4 && digits.saturating_mul(2) >= chars
}

#[cfg(test)]
mod tests {
    use super::redact_window_title;
    use crate::config::ObservationTitleMode;

    #[test]
    fn app_only_omits_document_title() {
        assert_eq!(
            redact_window_title(
                "請求書_20240821.pdf - Firefox",
                ObservationTitleMode::AppOnly
            ),
            "Firefox"
        );
        assert!(
            redact_window_title(
                "https://secret.example.jp/inbox",
                ObservationTitleMode::AppOnly
            )
            .is_empty()
        );
    }

    #[test]
    fn redacted_title_strips_japanese_path_url_email_and_ids() {
        let out = redact_window_title(
            "メモ https://example.jp/a user@example.com ID-20240821 ~/書類/秘密.txt Firefox",
            ObservationTitleMode::RedactedTitle,
        );
        assert_eq!(out, "メモ Firefox");
        assert!(!out.contains("example"));
        assert!(!out.contains('@'));
        assert!(!out.contains("20240821"));
        assert!(!out.contains("書類"));
    }

    #[test]
    fn full_title_keeps_raw_string() {
        let raw = "user@example.com ~/tmp";
        assert_eq!(
            redact_window_title(raw, ObservationTitleMode::FullTitle),
            raw
        );
    }
}
