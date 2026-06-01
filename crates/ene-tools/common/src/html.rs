use ego_tree::NodeId;
use scraper::{ElementRef, Html, Node, Selector};

const SKIP_TAGS: &[&str] = &[
    "script", "style", "noscript", "iframe", "svg", "nav", "header", "footer", "aside", "template",
    "code", "canvas", "audio", "video", "map", "object", "embed",
];

/// Converts raw HTML to Markdown text.
pub fn html_to_markdown(html: &str) -> String {
    htmd::convert(html).unwrap_or_default()
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

    let root = document.tree.get(target_id).unwrap();
    ElementRef::wrap(root).unwrap().html()
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

    let md = htmd::convert(&html_input).unwrap_or_default();
    normalize_text(&md)
}

fn select_target_id(html: &Html, extract: &str) -> NodeId {
    match extract {
        "main" => {
            let sel = Selector::parse("main").unwrap();
            if let Some(el) = html.select(&sel).next() {
                return el.id();
            }
            let sel = Selector::parse("body").unwrap();
            if let Some(el) = html.select(&sel).next() {
                return el.id();
            }
            html.root_element().id()
        }
        "body" => {
            let sel = Selector::parse("body").unwrap();
            if let Some(el) = html.select(&sel).next() {
                return el.id();
            }
            html.root_element().id()
        }
        _ => html.root_element().id(),
    }
}

fn strip_subtrees(tree: &mut ego_tree::Tree<Node>, root_id: NodeId, skip_tags: &[&str]) {
    let ids: Vec<NodeId> = tree
        .get(root_id)
        .unwrap()
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
    let re_multispace = regex::Regex::new(r"[ \t]+").unwrap();
    let re_multiline = regex::Regex::new(r"\n[ \t]*\n[ \t\n]*").unwrap();
    let re_leading_space = regex::Regex::new(r"[ \t]*\n[ \t]*").unwrap();

    let step1 = re_multispace.replace_all(text, " ");
    let step2 = re_multiline.replace_all(&step1, "\n\n");
    let step3 = re_leading_space.replace_all(&step2, "\n");

    step3.trim().to_string()
}
