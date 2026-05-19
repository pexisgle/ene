pub fn html_to_text(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    let mut skip_script = false;
    let mut tag_name = String::new();

    for c in html.chars() {
        if c == '<' {
            in_tag = true;
            tag_name.clear();
            continue;
        }
        if c == '>' {
            in_tag = false;
            let name = tag_name.to_lowercase();
            if name.starts_with("script") || name.starts_with("style") {
                skip_script = true;
            } else if name.starts_with("/script") || name.starts_with("/style") {
                skip_script = false;
            }
            if name == "br"
                || name == "br/"
                || name == "p"
                || name == "/p"
                || name == "div"
                || name == "/div"
            {
                text.push('\n');
            }
            continue;
        }
        if in_tag {
            if c.is_alphanumeric() || c == '/' {
                tag_name.push(c);
            }
            continue;
        }
        if !skip_script {
            text.push(c);
        }
    }

    let lines: Vec<&str> = text.lines().collect();
    let cleaned: Vec<String> = lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    cleaned.join("\n")
}

pub fn html_to_markdown(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| html_to_text(html))
}
