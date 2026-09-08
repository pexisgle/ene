use scraper::{ElementRef, Html, Node, Selector};
use serde_json::{Value, json};

pub const MAX_CONVERT_CHARS: usize = 64 * 1024;

const SKIP_TAGS: &[&str] = &["script", "style", "nav", "noscript"];

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
        let title = page_title(body);
        let markdown = html_to_markdown(body);
        let text = html_to_text(body);
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
    let document = Html::parse_document(html);
    let selector = Selector::parse("title").ok()?;
    let title = document
        .select(&selector)
        .next()
        .map(|element| collapse_ws(&element.text().collect::<String>()))?;
    if title.is_empty() { None } else { Some(title) }
}

pub(crate) fn html_to_markdown(html: &str) -> String {
    let converter = htmd::HtmlToMarkdown::builder()
        .skip_tags(SKIP_TAGS.to_vec())
        .build();
    converter.convert(html).unwrap_or_default()
}

fn html_to_text(html: &str) -> String {
    let document = Html::parse_document(html);
    let mut out = String::new();
    collect_text(document.root_element(), &mut out);
    collapse_ws(&out)
}

fn collect_text(element: ElementRef<'_>, out: &mut String) {
    if SKIP_TAGS.contains(&element.value().name()) {
        return;
    }
    for child in element.children() {
        match child.value() {
            Node::Text(text) => out.push_str(text),
            Node::Element(_) => {
                if let Some(child_el) = ElementRef::wrap(child) {
                    collect_text(child_el, out);
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn strip_tags(html: &str) -> String {
    let fragment = Html::parse_fragment(html);
    collapse_ws(&fragment.root_element().text().collect::<String>())
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
        let md = html_to_markdown(ARTICLE);
        assert!(md.contains("# City"), "{md}");
        assert!(
            md.contains("[Tokyo](https://example.invalid/tokyo)"),
            "{md}"
        );
        assert!(
            md.lines().any(|line| {
                let trimmed = line.trim_start();
                (trimmed.starts_with('-') || trimmed.starts_with('*')) && trimmed.contains("trains")
            }),
            "{md}"
        );
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

    #[test]
    fn entities_and_malformed_markup_still_convert() {
        let html = r"<html><head><title>A &amp; B</title></head>
            <body><!-- <h1>hidden</h1> --><h1>Ok</h1>
            <p>See <a href=https://example.invalid/x>here</a> &quot;quoted&quot;.</p>
            <div><div><p>nested</p></div></div>
            <p>unclosed";
        let md = html_to_markdown(html);
        assert!(md.contains("# Ok"), "{md}");
        assert!(!md.contains("hidden"), "{md}");
        assert!(md.contains("[here](https://example.invalid/x)"), "{md}");
        assert!(md.contains("quoted") || md.contains('"'), "{md}");
        assert!(md.contains("nested"), "{md}");
        assert_eq!(page_title(html).as_deref(), Some("A & B"));
    }
}
