use scraper::{ElementRef, Html, Node};

pub fn extract_html(html: &str, extract: &str, trim: bool) -> String {
    let document = Html::parse_document(html);
    let target = select_target_element(&document, extract);

    if trim {
        serialize_element_clean(target)
    } else {
        target.html()
    }
}

pub fn extract_markdown(html: &str, extract: &str, trim: bool) -> String {
    let html_input = if trim || extract != "full" {
        extract_html(html, extract, trim)
    } else {
        html.to_string()
    };

    let md = htmd::convert(&html_input).unwrap_or_default();
    normalize_text(&md)
}

fn select_target_element<'a>(document: &'a Html, extract: &str) -> ElementRef<'a> {
    match extract {
        "main" => {
            let sel = scraper::Selector::parse("main").unwrap();
            document
                .select(&sel)
                .next()
                .or_else(|| {
                    let sel = scraper::Selector::parse("body").unwrap();
                    document.select(&sel).next()
                })
                .unwrap_or(document.root_element())
        }
        "body" => {
            let sel = scraper::Selector::parse("body").unwrap();
            document
                .select(&sel)
                .next()
                .unwrap_or(document.root_element())
        }
        _ => document.root_element(),
    }
}

fn serialize_element_clean(element: ElementRef) -> String {
    const SKIP_TAGS: &[&str] = &[
        "script", "style", "noscript", "iframe", "svg", "nav", "header", "footer", "aside",
        "template", "code", "canvas", "audio", "video", "map", "object", "embed",
    ];

    let tag = element.value().name();
    if SKIP_TAGS.contains(&tag) {
        return String::new();
    }

    let mut result = String::new();
    result.push('<');
    result.push_str(tag);

    for (name, value) in element.value().attrs.iter() {
        result.push(' ');
        result.push_str(&name.local);
        result.push_str("=\"");
        result.push_str(value);
        result.push('"');
    }
    result.push('>');

    for child in element.children() {
        match child.value() {
            Node::Text(text) => result.push_str(text),
            Node::Element(_) => {
                if let Some(child_el) = ElementRef::wrap(child) {
                    result.push_str(&serialize_element_clean(child_el));
                }
            }
            _ => {}
        }
    }

    result.push_str("</");
    result.push_str(tag);
    result.push('>');

    result
}

fn normalize_text(text: &str) -> String {
    let re_multispace = regex::Regex::new(r"[ \t]+").unwrap();
    let re_multiline = regex::Regex::new(r"\n[ \t]*\n[ \t\n]*").unwrap();
    let re_leading_space = regex::Regex::new(r"[ \t]*\n[ \t]*").unwrap();

    let step1 = re_multispace.replace_all(text, " ");
    let step2 = re_multiline.replace_all(&step1, "\n\n");
    let step3 = re_leading_space.replace_all(&step2, "\n");

    step3.trim().to_string()
}

pub fn truncate_text(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        text.to_string()
    } else {
        let byte_end = text
            .char_indices()
            .nth(max_chars)
            .map(|(i, _)| i)
            .unwrap_or(text.len());
        format!(
            "{}\n\n[... truncated, total {} chars ...]",
            &text[..byte_end],
            char_count
        )
    }
}
