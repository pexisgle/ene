use ene_session::InnerAspect;

#[must_use]
pub const fn model_visible_for(aspect: InnerAspect) -> bool {
    !matches!(aspect, InnerAspect::Emotion)
}

#[must_use]
pub fn split_surface_and_inner(raw: &str) -> (String, Vec<(InnerAspect, String)>) {
    let mut speech = String::new();
    let mut inner = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find("<inner") {
        speech.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(tag_end) = after.find('>') else {
            speech.push_str(after);
            break;
        };
        let header = &after[..tag_end];
        let aspect = parse_aspect_attr(header).unwrap_or(InnerAspect::Thought);
        let body_start = tag_end + 1;
        if let Some(close_rel) = after[body_start..].find("</inner>") {
            let body = after[body_start..body_start + close_rel].trim();
            if !body.is_empty() {
                inner.push((aspect, body.to_owned()));
            }
            rest = &after[body_start + close_rel + "</inner>".len()..];
        } else {
            speech.push_str(after);
            rest = "";
            break;
        }
    }
    if !rest.is_empty() && !rest.starts_with("<inner") {
        speech.push_str(rest);
    }
    (collapse_ws(&speech), inner)
}

fn parse_aspect_attr(header: &str) -> Option<InnerAspect> {
    let key = "aspect=\"";
    let idx = header.find(key)?;
    let rest = &header[idx + key.len()..];
    let end = rest.find('"')?;
    match rest[..end].trim() {
        "thought" => Some(InnerAspect::Thought),
        "emotion" => Some(InnerAspect::Emotion),
        "action_intent" => Some(InnerAspect::ActionIntent),
        _ => None,
    }
}

fn collapse_ws(text: &str) -> String {
    let trimmed = text.trim();
    let mut out = String::new();
    let mut prev_space = false;
    for ch in trimmed.chars() {
        if ch.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else {
            prev_space = false;
            out.push(ch);
        }
    }
    out
}

#[must_use]
pub fn derive_thought_from_thinking(
    inner: &[(InnerAspect, String)],
    thinking: Option<&str>,
    enabled: bool,
) -> Option<(InnerAspect, String)> {
    if !enabled {
        return None;
    }
    if inner
        .iter()
        .any(|(aspect, _)| *aspect == InnerAspect::Thought)
    {
        return None;
    }
    let thinking = thinking.map(str::trim).filter(|text| !text.is_empty())?;
    Some((InnerAspect::Thought, thinking.to_owned()))
}
