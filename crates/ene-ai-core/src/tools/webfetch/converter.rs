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
    let mut md = String::new();
    let mut in_tag = false;
    let mut tag_name = String::new();
    let mut attrs = String::new();
    let mut skip_script = false;

    for c in html.chars() {
        if c == '<' {
            in_tag = true;
            tag_name.clear();
            attrs.clear();
            continue;
        }
        if c == '>' {
            in_tag = false;
            let full_tag = tag_name.to_lowercase();
            let name: String = full_tag
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '/')
                .collect();

            if name.starts_with("script") || name.starts_with("style") {
                skip_script = true;
            } else if name.starts_with("/script") || name.starts_with("/style") {
                skip_script = false;
            }

            if skip_script {
                continue;
            }

            match name.as_str() {
                "h1" => md.push_str("\n# "),
                "/h1" => md.push('\n'),
                "h2" => md.push_str("\n## "),
                "/h2" => md.push('\n'),
                "h3" => md.push_str("\n### "),
                "/h3" => md.push('\n'),
                "h4" => md.push_str("\n#### "),
                "/h4" => md.push('\n'),
                "p" | "div" => md.push('\n'),
                "/p" | "/div" => md.push('\n'),
                "br" | "br/" => md.push('\n'),
                "strong" | "b" => md.push_str("**"),
                "/strong" | "/b" => md.push_str("**"),
                "em" | "i" => md.push('*'),
                "/em" | "/i" => md.push('*'),
                "code" => md.push('`'),
                "/code" => md.push('`'),
                "pre" => md.push_str("\n```\n"),
                "/pre" => md.push_str("\n```\n"),
                "ul" => md.push('\n'),
                "/ul" => md.push('\n'),
                "ol" => md.push('\n'),
                "/ol" => md.push('\n'),
                "li" => md.push_str("- "),
                "/li" => md.push('\n'),
                "a" => {
                    if let Some(href_start) = attrs.find("href=\"") {
                        let rest = &attrs[href_start + 7..];
                        if let Some(_end) = rest.find('"') {
                            let _href = &rest[.._end];
                            md.push('[');
                        }
                    }
                }
                "/a" => md.push_str("]"),
                _ => {}
            }
            continue;
        }
        if in_tag {
            if tag_name.is_empty() && c.is_alphabetic()
                || tag_name.starts_with('/') && c.is_alphabetic()
            {
                tag_name.push(c);
            } else if !tag_name.is_empty() {
                attrs.push(c);
            } else if c == '/' {
                tag_name.push(c);
            }
            continue;
        }
        if !skip_script {
            md.push(c);
        }
    }

    let lines: Vec<&str> = md.lines().collect();
    let cleaned: Vec<String> = lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    cleaned.join("\n")
}
