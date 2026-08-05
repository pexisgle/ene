use ego_tree::NodeId;
use scraper::{ElementRef, Html, Node, Selector};
use std::sync::LazyLock;

const SKIP_TAGS: &[&str] = &[
    "script", "style", "noscript", "iframe", "svg", "nav", "header", "footer", "aside", "template",
    "canvas", "audio", "video", "map", "object", "embed",
];

/// Converts raw HTML to Markdown text.
///
/// If the underlying `htmd` converter fails (e.g. on severely malformed
/// input), the original HTML is returned as plain text so the caller still
/// gets a non-empty result instead of an empty string that silently drops
/// the page content.
pub fn html_to_markdown(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|e| {
        tracing::warn!(component = "html_to_markdown", error = %e, "htmd conversion failed, falling back to raw HTML");
        html.to_string()
    })
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
/// # Performance
///
/// This function parses the HTML document twice: once in `extract_html`
/// (via `scraper`) and again in `htmd::convert`. The double-parse is
/// unavoidable because `htmd` 0.x only accepts a `&str` input and does
/// not expose an API that accepts a pre-parsed DOM tree. For typical
/// page sizes the cost is acceptable, but callers processing very large
/// documents in a tight loop should be aware of the overhead.
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
    static RE_MULTISPACE: LazyLock<regex::Regex> = LazyLock::new(|| {
        #[expect(
            clippy::expect_used,
            reason = "constant regex pattern compiled once at first use"
        )]
        regex::Regex::new(r"[ \t]+").expect("invalid constant regex")
    });
    // Strip trailing whitespace before newlines without touching leading
    // indentation on the following line (preserves code-block structure).
    static RE_TRAILING_WS: LazyLock<regex::Regex> = LazyLock::new(|| {
        #[expect(
            clippy::expect_used,
            reason = "constant regex pattern compiled once at first use"
        )]
        regex::Regex::new(r"[ \t]+\n").expect("invalid constant regex")
    });
    static RE_EXCESSIVE_BLANKS: LazyLock<regex::Regex> = LazyLock::new(|| {
        #[expect(
            clippy::expect_used,
            reason = "constant regex pattern compiled once at first use"
        )]
        regex::Regex::new(r"\n{3,}").expect("invalid constant regex")
    });

    let step1 = RE_MULTISPACE.replace_all(text, " ");
    let step2 = RE_TRAILING_WS.replace_all(&step1, "\n");
    let step3 = RE_EXCESSIVE_BLANKS.replace_all(&step2, "\n\n");

    step3.trim().to_string()
}
