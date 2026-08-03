use ene_plugin::prelude::*;
use std::sync::Arc;

const DEFAULT_LINE_LIMIT: usize = 2000;
const MAX_LINE_LENGTH: usize = 2000;
/// Mirrors `fs.read`'s default `max_read_bytes` (50 KiB) so one
/// `get_content` window costs the LLM context no more than one file read.
/// The window is cut to fit, keeping `offset` continuation line-based.
const MAX_OUTPUT_BYTES: usize = 50 * 1024;
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
    /// 1-indexed line number to start reading from (0 is treated as line 1).
    #[serde(default)]
    offset: Option<u64>,
    /// Maximum number of lines to return (default 2000, must be at least 1).
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
/// a window that does not reach the end of `content` is followed by a
/// footer stating the omitted range and the exact offset to resume, and an
/// offset past the last line is rejected. The default full read returns
/// `content` byte-identical; windowed or truncated reads rejoin lines with
/// `\n`, so CR/CRLF sources are normalized there. Lines longer than
/// [`MAX_LINE_LENGTH`] chars are cut with a suffix; a cut line cannot be
/// resumed through the line-based offset, so the footer says so. Windows
/// are additionally capped at [`MAX_OUTPUT_BYTES`].
fn paginate_content(
    content: &str,
    offset: Option<u64>,
    limit: Option<u64>,
) -> Result<String, ToolError> {
    if limit == Some(0) {
        return Err(ToolError::InvalidArguments {
            message: "limit must be at least 1".into(),
        });
    }

    let lines: Vec<&str> = content.lines().collect();
    let start = offset
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(1)
        .saturating_sub(1);
    if start >= lines.len() && !(lines.is_empty() && start == 0) {
        return Err(ToolError::execution_failed(format!(
            "Offset {} is out of range for this content ({} lines)",
            start + 1,
            lines.len()
        )));
    }
    let requested_end = start
        .saturating_add(
            limit
                .and_then(|v| usize::try_from(v).ok())
                .unwrap_or(DEFAULT_LINE_LIMIT),
        )
        .min(lines.len());

    let mut output = String::new();
    let mut end = start;
    let mut used_bytes = 0usize;
    let mut first_truncated_line = None;
    let mut truncated_line_count = 0usize;
    let mut byte_capped = false;
    for (index, line) in lines[start..requested_end].iter().enumerate() {
        let line_no = start + index + 1;
        let rendered = if line.chars().count() > MAX_LINE_LENGTH {
            if first_truncated_line.is_none() {
                first_truncated_line = Some(line_no);
            }
            truncated_line_count += 1;
            let head: String = line.chars().take(MAX_LINE_LENGTH).collect();
            format!("{head}{MAX_LINE_SUFFIX}")
        } else {
            (*line).to_string()
        };
        let added = rendered.len() + 1;
        if used_bytes + added > MAX_OUTPUT_BYTES {
            byte_capped = true;
            break;
        }
        output.push_str(&rendered);
        output.push('\n');
        used_bytes += added;
        end = line_no;
    }

    if start == 0 && end == lines.len() && first_truncated_line.is_none() {
        return Ok(content.to_string());
    }

    let mut footers = Vec::new();
    if let Some(line_no) = first_truncated_line {
        let note = if truncated_line_count == 1 {
            format!(
                "(Line {line_no} exceeds {MAX_LINE_LENGTH} chars and was cut; \
                 line-based offset cannot resume it. Use a narrower extract scope \
                 or format to read the rest.)"
            )
        } else {
            format!(
                "(Lines exceeding {MAX_LINE_LENGTH} chars were cut, first at line {line_no}; \
                 line-based offset cannot resume them. Use a narrower extract scope \
                 or format to read the rest.)"
            )
        };
        footers.push(note);
    }
    if end < lines.len() {
        let note = if byte_capped {
            format!(
                "(Output capped at {} KiB. Showing lines {}-{} of {}. Use offset={} to continue.)",
                MAX_OUTPUT_BYTES / 1024,
                start + 1,
                end,
                lines.len(),
                end + 1
            )
        } else {
            format!(
                "(Showing lines {}-{} of {}. Use offset={} to continue.)",
                start + 1,
                end,
                lines.len(),
                end + 1
            )
        };
        footers.push(note);
    }
    for footer in footers {
        output.push('\n');
        output.push_str(&footer);
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
    fn paginate_returns_full_content_byte_identical() {
        let result = paginate_content("a\nb\nc", None, None).unwrap();
        assert_eq!(result, "a\nb\nc");
    }

    #[test]
    fn paginate_preserves_crlf_when_not_windowing() {
        let result = paginate_content("a\r\nb\r\nc", None, None).unwrap();
        assert_eq!(result, "a\r\nb\r\nc");
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
    fn paginate_clamps_offset_zero_to_line_one() {
        let result = paginate_content("a\nb\nc", Some(0), None).unwrap();
        assert_eq!(result, "a\nb\nc");
    }

    #[test]
    fn paginate_rejects_offset_past_last_line() {
        for offset in [3, 4] {
            let result = paginate_content("a\nb", Some(offset), None);
            assert_eq!(
                result.unwrap_err().to_string(),
                format!(
                    "Execution failed: Offset {offset} is out of range for this content (2 lines)"
                )
            );
        }
    }

    #[test]
    fn paginate_rejects_zero_limit() {
        let result = paginate_content("a\nb", None, Some(0));
        assert!(
            matches!(result, Err(ToolError::InvalidArguments { .. })),
            "limit=0 must be rejected, got: {result:?}"
        );
    }

    #[test]
    fn paginate_huge_limit_does_not_panic() {
        let result = paginate_content("a\nb\nc", Some(1), Some(u64::MAX)).unwrap();
        assert_eq!(result, "a\nb\nc");
    }

    #[test]
    fn paginate_default_limit_windows_large_content() {
        let content = (1..=2500)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let result = paginate_content(&content, None, None).unwrap();
        assert_eq!(result.lines().count(), 2002);
        assert_eq!(
            result.lines().last(),
            Some("(Showing lines 1-2000 of 2500. Use offset=2001 to continue.)")
        );
    }

    #[test]
    fn paginate_truncated_line_reports_no_resume_path() {
        let long = "x".repeat(MAX_LINE_LENGTH + 10);
        let result = paginate_content(&long, None, None).unwrap();
        let head = format!("{}{MAX_LINE_SUFFIX}", "x".repeat(MAX_LINE_LENGTH));
        let expected = format!(
            "{head}\n\n(Line 1 exceeds {MAX_LINE_LENGTH} chars and was cut; \
             line-based offset cannot resume it. Use a narrower extract scope \
             or format to read the rest.)"
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn paginate_truncated_lines_still_footers_continuation() {
        let long = "x".repeat(MAX_LINE_LENGTH + 10);
        let content = format!("{long}\nline2\nline3");
        let result = paginate_content(&content, None, Some(2)).unwrap();
        let expected = format!(
            "{}{MAX_LINE_SUFFIX}\nline2\n\n(Line 1 exceeds {MAX_LINE_LENGTH} chars and was cut; \
             line-based offset cannot resume it. Use a narrower extract scope \
             or format to read the rest.)\n\
             (Showing lines 1-2 of 3. Use offset=3 to continue.)",
            "x".repeat(MAX_LINE_LENGTH)
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn paginate_byte_caps_window() {
        let line = "x".repeat(MAX_LINE_LENGTH);
        let content = std::iter::repeat_n(line.as_str(), 40)
            .collect::<Vec<_>>()
            .join("\n");
        let result = paginate_content(&content, None, None).unwrap();
        // 25 lines of 2000 chars + '\n' fit in 50 KiB; the 26th would exceed it.
        let body = format!("{}\n", "x".repeat(MAX_LINE_LENGTH)).repeat(25);
        let expected = format!(
            "{body}\n(Output capped at {} KiB. Showing lines 1-25 of 40. Use offset=26 to continue.)",
            MAX_OUTPUT_BYTES / 1024
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn paginate_empty_content_returns_empty() {
        let result = paginate_content("", None, None).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn paginate_deserializes_offset_and_limit() {
        let action: GetContentAction = serde_json::from_str(r#"{"offset":2,"limit":3}"#).unwrap();
        assert_eq!(action.offset, Some(2));
        assert_eq!(action.limit, Some(3));
        let action: GetContentAction = serde_json::from_str(r#"{"format":"html"}"#).unwrap();
        assert_eq!(action.offset, None);
        assert_eq!(action.limit, None);
    }
}
