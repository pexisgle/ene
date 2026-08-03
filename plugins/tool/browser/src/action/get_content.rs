use ene_plugin::prelude::*;
use std::fmt::Write;
use std::sync::Arc;

const DEFAULT_LINE_LIMIT: usize = 2000;
const MAX_LINE_LENGTH: usize = 2000;
const MAX_LINE_SUFFIX: &str = "... (line truncated)";

#[derive(Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "browser",
    name = "get_content",
    summary = "Gets structural page content formatted as Markdown or HTML.",
    category = "Browser",
    keywords_primary = "content, dom, html, markdown"
)]
pub struct GetContentAction {
    /// Output format (default: 'markdown'). 'markdown' preserves
    /// headings/links/lists as Markdown, 'html' returns raw HTML.
    #[arg(enum_values = "markdown, html")]
    format: Option<String>,
    /// Extraction scope (default: 'body'). 'body' = `<body>` content,
    /// 'main' = `<main>` content (falls back to `<body>`), 'full' = entire
    /// document including `<head>`.
    #[arg(enum_values = "body, main, full")]
    extract: Option<String>,
    /// Remove non-content elements (default: true). When true, removes:
    /// script, style, noscript, iframe, svg, nav, header, footer, aside,
    /// template, code, canvas, audio, video, map, object, embed.
    trim: Option<bool>,
    /// 1-indexed line number to start reading from.
    #[serde(default)]
    offset: Option<u64>,
    /// Maximum number of lines to return (default 2000).
    #[serde(default)]
    limit: Option<u64>,

    #[tool(skip)]
    #[serde(skip, default = "crate::utils::default_store")]
    store: Arc<crate::utils::session::BrowserSessionStore>,
}

impl GetContentAction {
    pub const fn new(store: Arc<crate::utils::session::BrowserSessionStore>) -> Self {
        Self {
            format: None,
            extract: None,
            trim: None,
            offset: None,
            limit: None,
            store,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let format = self.format.as_deref().unwrap_or("markdown");
        let extract = self.extract.as_deref().unwrap_or("body");
        let trim = self.trim.unwrap_or(true);

        match format {
            "markdown" | "html" => {}
            other => {
                return Err(ToolError::InvalidArguments {
                    message: format!("Invalid format '{other}'. Valid values: markdown, html"),
                });
            }
        }
        match extract {
            "body" | "main" | "full" => {}
            other => {
                return Err(ToolError::InvalidArguments {
                    message: format!("Invalid extract '{other}'. Valid values: body, main, full"),
                });
            }
        }

        let page = self.store.acquire_page().await?;

        let html = page
            .content()
            .await
            .map_err(|e| ToolError::execution_failed(format!("Failed to get content: {e}")))?;

        let extracted = match format {
            "html" => ene_util::html::extract_html(&html, extract, trim),
            _ => ene_util::html::extract_markdown(&html, extract, trim),
        };

        paginate_content(&extracted, self.offset, self.limit)
    }
}

/// Windows `content` by line, mirroring `fs.read`'s continuation protocol:
/// when the window does not cover the whole content, the returned text ends
/// with a footer stating the omitted range and the exact offset to resume.
fn paginate_content(
    content: &str,
    offset: Option<u64>,
    limit: Option<u64>,
) -> Result<String, ToolError> {
    let lines: Vec<&str> = content.lines().collect();
    let start = offset
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(1)
        .saturating_sub(1);
    if start > lines.len() && !(lines.is_empty() && start == 0) {
        return Err(ToolError::execution_failed(format!(
            "Offset {} is out of range for this content ({} lines)",
            start + 1,
            lines.len()
        )));
    }
    let end = (start
        + limit
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(DEFAULT_LINE_LIMIT))
    .min(lines.len());

    let mut output = String::new();
    for line in &lines[start..end] {
        let line = if line.chars().count() > MAX_LINE_LENGTH {
            let head: String = line.chars().take(MAX_LINE_LENGTH).collect();
            format!("{head}{MAX_LINE_SUFFIX}")
        } else {
            (*line).to_string()
        };
        output.push_str(&line);
        output.push('\n');
    }

    if end < lines.len() {
        // `fmt::Error` is `Copy`, so `drop()` would itself trip
        // `clippy::dropping_copy_types`; writing into a `String` via
        // `fmt::Write` never actually fails.
        #[expect(
            clippy::let_underscore_must_use,
            reason = "fmt::Write to a String is infallible in practice"
        )]
        let _ = write!(
            output,
            "\n(Showing lines {}-{} of {}. Use offset={} to continue.)",
            start + 1,
            end,
            lines.len(),
            end + 1
        );
    }

    Ok(output)
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test fixtures assert on the paginated output directly"
)]
mod tests {
    use super::*;

    fn action() -> GetContentAction {
        GetContentAction::new(crate::utils::default_store())
    }

    #[tokio::test]
    async fn rejects_unknown_format() {
        let result = action().execute(r#"{"format":"htm"}"#).await;
        assert!(
            matches!(result, Err(ToolError::InvalidArguments { .. })),
            "unknown format must be rejected, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn rejects_unknown_extract() {
        let result = action().execute(r#"{"extract":"sidebar"}"#).await;
        assert!(
            matches!(result, Err(ToolError::InvalidArguments { .. })),
            "unknown extract must be rejected, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn rejects_case_variant_format() {
        let result = action().execute(r#"{"format":"Markdown"}"#).await;
        assert!(
            matches!(result, Err(ToolError::InvalidArguments { .. })),
            "case-variant format must be rejected"
        );
    }

    #[test]
    fn paginate_returns_full_content_when_within_limit() {
        let result = paginate_content("a\nb\nc", None, None).unwrap();
        assert_eq!(result, "a\nb\nc\n");
    }

    #[test]
    fn paginate_appends_continuation_footer_when_truncated() {
        let content = (1..=5)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let result = paginate_content(&content, None, Some(3)).unwrap();
        assert_eq!(
            result,
            "line1\nline2\nline3\n\n(Showing lines 1-3 of 5. Use offset=4 to continue.)"
        );
    }

    #[test]
    fn paginate_windows_from_offset() {
        let content = (1..=6)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let result = paginate_content(&content, Some(4), Some(2)).unwrap();
        assert_eq!(
            result,
            "line4\nline5\n\n(Showing lines 4-5 of 6. Use offset=6 to continue.)"
        );
    }

    #[test]
    fn paginate_returns_window_without_footer_at_end() {
        let content = (1..=4)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let result = paginate_content(&content, Some(3), None).unwrap();
        assert_eq!(result, "line3\nline4\n");
    }

    #[test]
    fn paginate_rejects_offset_beyond_content() {
        let result = paginate_content("a\nb", Some(5), None);
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("out of range"),
            "out-of-range offset must explain itself, got: {err:?}"
        );
    }

    #[test]
    fn paginate_truncates_pathological_lines() {
        let long = "x".repeat(MAX_LINE_LENGTH + 10);
        let result = paginate_content(&long, None, None).unwrap();
        assert_eq!(
            result.chars().count(),
            MAX_LINE_LENGTH + MAX_LINE_SUFFIX.chars().count() + 1
        );
        assert!(result.ends_with(&format!("{MAX_LINE_SUFFIX}\n")));
    }

    #[test]
    fn paginate_empty_content_returns_empty() {
        let result = paginate_content("", None, None).unwrap();
        assert_eq!(result, "");
    }
}
