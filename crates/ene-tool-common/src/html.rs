use ego_tree::NodeId;
use scraper::{ElementRef, Html, Node, Selector};
use std::sync::OnceLock;

const SKIP_TAGS: &[&str] = &[
    "script", "style", "noscript", "iframe", "svg", "nav", "header", "footer", "aside", "template",
    "code", "canvas", "audio", "video", "map", "object", "embed",
];

/// Converts raw HTML to Markdown text.
///
/// If the underlying `htmd` converter fails (e.g. on severely malformed
/// input), the original HTML is returned as plain text so the caller still
/// gets a non-empty result instead of an empty string. The previous
/// `unwrap_or_default()` returned an empty string on every failure, which
/// silently dropped the page content.
pub fn html_to_markdown(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| html.to_string())
}

/// Extracts a specific region from HTML and returns it as HTML.
///
/// * `extract` — Target selector: `"body"`, `"main"`, or `"full"`
/// * `trim` — If true, removes non-semantic HTML noise (scripts, styles, etc.)
pub fn extract_html(html: &str, extract: &str, trim: bool) -> String {
    let mut document = Html::parse_document(html);
    let target_id = select_target_id(&document, extract);

    if trim {
        strip_subtrees(&mut document.tree, target_id, SKIP_TAGS);
    }

    if let Some(root) = document.tree.get(target_id)
        && let Some(el) = ElementRef::wrap(root)
    {
        el.html()
    } else {
        String::new()
    }
}

/// Extracts and converts a specific region of HTML to Markdown.
///
/// Applies `extract_html` first, then converts the result to Markdown.
pub fn extract_markdown(html: &str, extract: &str, trim: bool) -> String {
    let html_input = if trim || extract != "full" {
        extract_html(html, extract, trim)
    } else {
        html.to_string()
    };

    let md = htmd::convert(&html_input).unwrap_or_else(|_| html_input.clone());
    normalize_text(&md)
}

fn select_target_id(html: &Html, extract: &str) -> NodeId {
    match extract {
        "main" => {
            if let Ok(sel) = Selector::parse("main")
                && let Some(el) = html.select(&sel).next()
            {
                return el.id();
            }
            if let Ok(sel) = Selector::parse("body")
                && let Some(el) = html.select(&sel).next()
            {
                return el.id();
            }
            html.root_element().id()
        }
        "body" => {
            if let Ok(sel) = Selector::parse("body")
                && let Some(el) = html.select(&sel).next()
            {
                return el.id();
            }
            html.root_element().id()
        }
        _ => html.root_element().id(),
    }
}

fn strip_subtrees(tree: &mut ego_tree::Tree<Node>, root_id: NodeId, skip_tags: &[&str]) {
    let Some(root_node) = tree.get(root_id) else {
        return;
    };
    let ids: Vec<NodeId> = root_node
        .descendants()
        .filter_map(|node| match node.value() {
            Node::Element(el) if skip_tags.contains(&el.name()) => Some(node.id()),
            _ => None,
        })
        .collect();

    for id in ids {
        if let Some(mut node) = tree.get_mut(id) {
            node.detach();
        }
    }
}

fn normalize_text(text: &str) -> String {
    static RE_MULTISPACE: OnceLock<regex::Regex> = OnceLock::new();
    static RE_MULTILINE: OnceLock<regex::Regex> = OnceLock::new();
    static RE_LEADING_SPACE: OnceLock<regex::Regex> = OnceLock::new();

    let re_multispace = RE_MULTISPACE.get_or_init(|| {
        #[expect(
            clippy::expect_used,
            reason = "constant regex pattern compiled once at first use"
        )]
        regex::Regex::new(r"[ \t]+").expect("invalid constant regex")
    });
    let re_multiline = RE_MULTILINE.get_or_init(|| {
        #[expect(
            clippy::expect_used,
            reason = "constant regex pattern compiled once at first use"
        )]
        regex::Regex::new(r"\n[ \t]*\n[ \t\n]*").expect("invalid constant regex")
    });
    let re_leading_space = RE_LEADING_SPACE.get_or_init(|| {
        #[expect(
            clippy::expect_used,
            reason = "constant regex pattern compiled once at first use"
        )]
        regex::Regex::new(r"[ \t]*\n[ \t]*").expect("invalid constant regex")
    });

    let step1 = re_multispace.replace_all(text, " ");
    let step2 = re_multiline.replace_all(&step1, "\n\n");
    let step3 = re_leading_space.replace_all(&step2, "\n");

    step3.trim().to_string()
}
