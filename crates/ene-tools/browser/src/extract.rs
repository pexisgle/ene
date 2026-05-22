pub fn extract_html(html: &str, extract: &str, trim: bool) -> String {
    ene_tools_common::html::extract_html(html, extract, trim)
}

pub fn extract_markdown(html: &str, extract: &str, trim: bool) -> String {
    ene_tools_common::html::extract_markdown(html, extract, trim)
}

pub fn truncate_text(text: &str, max_chars: usize) -> String {
    ene_tools_common::truncate::Truncate::chars(text, max_chars)
}
