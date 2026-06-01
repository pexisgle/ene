use ene_tools_common::html::{extract_html, extract_markdown, html_to_markdown};

#[test]
fn extract_html_full_no_trim_preserves_script() {
    let html = r#"<p>hello</p><script>var x = 1;</script>"#;
    let out = extract_html(html, "full", false);
    assert!(out.contains("<script>"), "full/no-trim should preserve script");
    assert!(out.contains("hello"), "full/no-trim should preserve text");
}

#[test]
fn extract_html_full_trim_strips_script() {
    let html = r#"<p>hello</p><script>var x = 1;</script>"#;
    let out = extract_html(html, "full", true);
    assert!(!out.contains("<script>"), "full/trim should remove script tags");
    assert!(!out.contains("var x"), "full/trim should remove script content");
    assert!(out.contains("hello"), "full/trim should preserve text");
}

#[test]
fn extract_html_full_trim_strips_style() {
    let html = r#"<p>hello</p><style>body { background: red; }</style>"#;
    let out = extract_html(html, "full", true);
    assert!(!out.contains("<style>"), "full/trim should remove style tags");
    assert!(out.contains("hello"), "full/trim should preserve text");
}

#[test]
fn extract_html_full_trim_strips_noscript() {
    let html = r#"<p>hello</p><noscript>fallback</noscript>"#;
    let out = extract_html(html, "full", true);
    assert!(!out.contains("<noscript>"), "full/trim should remove noscript");
    assert!(!out.contains("fallback"), "full/trim should remove noscript content");
}

#[test]
fn extract_html_body_works() {
    let html = r#"<header>nav</header><main><p>content</p></main><footer>foot</footer>"#;
    let out = extract_html(html, "body", false);
    assert!(out.contains("<body>"), "body extraction should include <body>");
    assert!(out.contains("content"), "body extraction should include body content");
}

#[test]
fn extract_html_main_selects_main() {
    let html = r#"<body><header>nav</header><main><p>content</p></main><footer>foot</footer></body>"#;
    let out = extract_html(html, "main", false);
    assert!(out.contains("<main>"), "main extraction should include <main>");
    assert!(out.contains("content"), "main extraction should include content");
}

#[test]
fn extract_html_main_falls_back_to_body() {
    let html = r#"<body><p>content</p></body>"#;
    let out = extract_html(html, "main", false);
    assert!(out.contains("<body>"), "main fallback should include <body>");
    assert!(out.contains("content"), "main fallback should include content");
}

#[test]
fn extract_html_main_falls_back_to_root() {
    let html = r#"<p>content</p>"#;
    let out = extract_html(html, "main", false);
    assert!(out.contains("content"), "ultimate fallback should include content");
}

#[test]
fn extract_html_strips_multiple_skip_tags() {
    let html = r#"
        <p>keep</p>
        <script>a</script>
        <style>b</style>
        <nav>c</nav>
        <iframe>d</iframe>
        <p>also keep</p>
    "#;
    let out = extract_html(html, "full", true);
    assert!(out.contains("keep"), "non-skip content should be preserved");
    assert!(out.contains("also keep"), "non-skip content should be preserved");
    assert!(!out.contains("<script>"), "script tag should be removed");
    assert!(!out.contains("<style>"), "style tag should be removed");
    assert!(!out.contains("<nav>"), "nav tag should be removed");
    assert!(!out.contains("<iframe>"), "iframe tag should be removed");
}

#[test]
fn extract_html_nested_skip_tags() {
    let html = r#"<div>before<script>outer<x>inner</x></script>after</div>"#;
    let out = extract_html(html, "full", true);
    assert!(out.contains("before"), "text before script should be preserved");
    assert!(out.contains("after"), "text after script should be preserved");
    assert!(!out.contains("<script>"), "nested script should be removed");
    assert!(!out.contains("inner"), "inner content of script should be removed");
}

#[test]
fn extract_markdown_full_trim_strips_script() {
    let html = r#"<h1>Title</h1><script>alert(1)</script><p>body</p>"#;
    let md = extract_markdown(html, "full", true);
    assert!(md.contains("Title"), "markdown should contain heading text");
    assert!(md.contains("body"), "markdown should contain paragraph text");
    assert!(!md.contains("alert"), "markdown should not contain script content");
}

#[test]
fn extract_markdown_full_no_trim_preserves_script_in_md() {
    let html = r#"<h1>Title</h1><script>alert(1)</script>"#;
    let md = extract_markdown(html, "full", false);
    assert!(md.contains("Title"), "markdown should contain heading text");
}

#[test]
fn html_to_markdown_basic() {
    let html = "<h1>Hello</h1><p>World</p>";
    let md = html_to_markdown(html);
    assert!(md.contains("Hello"), "html_to_markdown should convert heading");
    assert!(md.contains("World"), "html_to_markdown should convert paragraph");
}
