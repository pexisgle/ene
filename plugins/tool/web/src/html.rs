use serde_json::{Value, json};

pub const MAX_CONVERT_CHARS: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FetchFormat {
    Markdown,
    Text,
    Html,
}

pub(crate) fn fail(code: &str, message: impl Into<String>) -> String {
    json!({ "code": code, "message": message.into() }).to_string()
}

pub(crate) fn parse_format(raw: Option<&str>) -> Result<FetchFormat, String> {
    match raw.unwrap_or("markdown") {
        "markdown" => Ok(FetchFormat::Markdown),
        "text" => Ok(FetchFormat::Text),
        "html" => Ok(FetchFormat::Html),
        other => Err(fail(
            "invalid_format",
            format!("format must be markdown, text, or html (got {other})"),
        )),
    }
}

pub(crate) fn is_binary(content_type: &str) -> bool {
    let main = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    main.starts_with("image/")
        || main.starts_with("audio/")
        || main.starts_with("video/")
        || main == "application/octet-stream"
        || main == "application/pdf"
        || main == "application/zip"
        || main == "application/gzip"
}

pub(crate) fn render(
    body: &str,
    content_type: &str,
    format: FetchFormat,
    url: &str,
) -> Result<Value, String> {
    if is_binary(content_type) {
        return Err(fail(
            "binary_content",
            format!("refusing to convert binary content_type {content_type}"),
        ));
    }
    let htmlish = content_type.to_ascii_lowercase().contains("html");
    let (title, markdown, text) = if htmlish {
        let readable = readable_html(body);
        let title = page_title(body);
        let markdown = html_to_markdown(&readable);
        let text = strip_tags(&readable);
        (title, markdown, text)
    } else {
        (None, body.to_owned(), body.to_owned())
    };
    let body_out = match format {
        FetchFormat::Html => body.to_owned(),
        FetchFormat::Markdown => markdown,
        FetchFormat::Text => text,
    };
    let body_out = cap_chars(&body_out);
    Ok(json!({
        "url": url,
        "title": title,
        "format": format_name(format),
        "content_type": content_type,
        "text": body_out,
        "chars": body_out.chars().count(),
    }))
}

fn format_name(format: FetchFormat) -> &'static str {
    match format {
        FetchFormat::Markdown => "markdown",
        FetchFormat::Text => "text",
        FetchFormat::Html => "html",
    }
}

fn cap_chars(text: &str) -> String {
    if text.chars().count() <= MAX_CONVERT_CHARS {
        return text.to_owned();
    }
    text.chars().take(MAX_CONVERT_CHARS).collect()
}

fn page_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let after = html.get(start..)?;
    let gt = after.find('>')?;
    let rest = after.get(gt + 1..)?;
    let end_rel = rest.to_ascii_lowercase().find("</title>")?;
    let raw = rest.get(..end_rel)?;
    let title = strip_tags(raw);
    if title.is_empty() { None } else { Some(title) }
}

fn readable_html(html: &str) -> String {
    let mut out = html.to_owned();
    for tag in ["script", "style", "nav", "noscript"] {
        out = strip_elements(&out, tag);
    }
    out
}

fn strip_elements(html: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut rest = html;
    let mut out = String::with_capacity(html.len());
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(start) = lower.find(&open) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after_open = &rest[start..];
        let Some(gt) = after_open.find('>') else {
            break;
        };
        let after_gt = &after_open[gt + 1..];
        let close_at = after_gt.to_ascii_lowercase().find(&close);
        rest = close_at.map_or("", |idx| after_gt.get(idx + close.len()..).unwrap_or(""));
    }
    out
}

pub(crate) fn html_to_markdown(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    let mut in_tag = false;
    let mut tag = String::new();
    let mut href: Option<String> = None;
    let mut link_text = String::new();
    let mut in_link = false;
    while let Some(ch) = rest.chars().next() {
        let n = ch.len_utf8();
        rest = &rest[n..];
        if ch == '<' {
            in_tag = true;
            tag.clear();
            continue;
        }
        if ch == '>' && in_tag {
            in_tag = false;
            let parsed = parse_tag(&tag);
            apply_tag(&parsed, &mut out, &mut href, &mut in_link, &mut link_text);
            continue;
        }
        if in_tag {
            tag.push(ch);
            continue;
        }
        if in_link {
            link_text.push(ch);
        } else {
            push_text(&mut out, ch);
        }
    }
    collapse_md(&out)
}

struct ParsedTag {
    name: String,
    closing: bool,
    href: Option<String>,
}

fn parse_tag(raw: &str) -> ParsedTag {
    let raw = raw.trim();
    let closing = raw.starts_with('/');
    let body = raw.strip_prefix('/').unwrap_or(raw);
    let name = body
        .split(|c: char| c.is_whitespace() || c == '/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let href = attr(body, "href");
    ParsedTag {
        name,
        closing,
        href,
    }
}

fn attr(tag: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    let lower = tag.to_ascii_lowercase();
    let at = lower.find(&needle)?;
    let rest = tag.get(at + needle.len()..)?.trim_start();
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let inner = rest.get(1..)?;
        let end = inner.find(quote)?;
        Some(inner[..end].to_owned())
    } else {
        Some(
            rest.split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches('/')
                .to_owned(),
        )
    }
}

fn apply_tag(
    tag: &ParsedTag,
    out: &mut String,
    href: &mut Option<String>,
    in_link: &mut bool,
    link_text: &mut String,
) {
    match (tag.name.as_str(), tag.closing) {
        ("br", _) => out.push('\n'),
        ("p" | "div" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li", true) => {
            out.push_str("\n\n");
        }
        ("p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "pre", false) => {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            match tag.name.as_str() {
                "h1" => out.push_str("# "),
                "h2" => out.push_str("## "),
                "h3" => out.push_str("### "),
                "h4" => out.push_str("#### "),
                "h5" => out.push_str("##### "),
                "h6" => out.push_str("###### "),
                "li" => out.push_str("- "),
                "pre" => out.push_str("```\n"),
                _ => {}
            }
        }
        ("pre", true) => out.push_str("\n```\n"),
        ("a", false) => {
            href.clone_from(&tag.href);
            *in_link = true;
            link_text.clear();
        }
        ("a", true) => {
            let text = collapse_ws(link_text);
            if let Some(url) = href.take()
                && !text.is_empty()
            {
                out.push('[');
                out.push_str(&text);
                out.push_str("](");
                out.push_str(&url);
                out.push(')');
            } else {
                out.push_str(&text);
            }
            *in_link = false;
            link_text.clear();
        }
        _ => {}
    }
}

fn push_text(out: &mut String, ch: char) {
    if ch == '\0' {
        return;
    }
    out.push(ch);
}

fn collapse_md(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = collapse_ws(line);
        if trimmed.is_empty() {
            if lines.last().is_none_or(|row| !row.is_empty()) {
                lines.push(String::new());
            }
        } else {
            lines.push(trimmed);
        }
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

pub(crate) fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    collapse_ws(&out)
}

fn collapse_ws(text: &str) -> String {
    let mut out = String::new();
    let mut prev_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            prev_space = false;
            out.push(c);
        }
    }
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::{FetchFormat, html_to_markdown, is_binary, page_title, render};

    const ARTICLE: &str = r#"<html><head><title>Tokyo notes</title>
        <script>secret()</script><style>body{color:red}</style></head>
        <body><nav>Home</nav>
        <h1>City</h1>
        <p>See <a href="https://example.invalid/tokyo">Tokyo</a> tonight.</p>
        <ul><li>trains</li><li>food</li></ul>
        </body></html>"#;

    #[test]
    fn markdown_keeps_headings_paragraphs_and_links() {
        let md = html_to_markdown(&super::readable_html(ARTICLE));
        assert!(md.contains("# City"), "{md}");
        assert!(
            md.contains("[Tokyo](https://example.invalid/tokyo)"),
            "{md}"
        );
        assert!(md.contains("- trains"), "{md}");
        assert!(!md.contains("secret()"), "{md}");
        assert!(!md.contains("Home"), "{md}");
        assert_eq!(page_title(ARTICLE).as_deref(), Some("Tokyo notes"));
    }

    #[test]
    fn render_markdown_includes_source_url_and_title() {
        let value = render(
            ARTICLE,
            "text/html; charset=utf-8",
            FetchFormat::Markdown,
            "https://example.invalid/a",
        )
        .unwrap();
        assert_eq!(value["url"], "https://example.invalid/a");
        assert_eq!(value["title"], "Tokyo notes");
        assert_eq!(value["format"], "markdown");
        let text = value["text"].as_str().unwrap();
        assert!(text.contains("# City"));
        assert!(text.contains("[Tokyo](https://example.invalid/tokyo)"));
    }

    #[test]
    fn binary_content_is_rejected() {
        assert!(is_binary("image/png"));
        assert!(is_binary("application/pdf"));
        assert!(!is_binary("text/html; charset=utf-8"));
        let err = render("x", "image/png", FetchFormat::Markdown, "https://x").unwrap_err();
        assert!(err.contains("binary_content"), "{err}");
    }
}
