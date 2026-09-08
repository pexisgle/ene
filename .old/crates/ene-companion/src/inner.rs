use ene_session::InnerAspect;

/// Default `model_visible` for an inner aspect (thought/action true, emotion false).
#[must_use]
pub const fn model_visible_for(aspect: InnerAspect) -> bool {
    !matches!(aspect, InnerAspect::Emotion)
}

/// Split surface speech from `<inner aspect="…">…</inner>` tags.
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
            rest = "";
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
    speech.push_str(rest);
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

/// Parse `emotion: label` or `emotion: label(0.8)` from an inner body.
#[must_use]
pub fn parse_emotion_report(body: &str) -> Option<EmotionReport> {
    let line = body
        .lines()
        .map(str::trim)
        .find(|line| line.to_ascii_lowercase().starts_with("emotion:"))
        .unwrap_or(body.trim());
    let rest = line.split_once(':').map_or(line, |(_, r)| r.trim()).trim();
    if rest.is_empty() {
        return None;
    }
    if let Some(paren) = rest.find('(') {
        let label = rest[..paren].trim();
        let inside = rest[paren + 1..].trim_end_matches(')').trim();
        let intensity = inside.parse::<f32>().ok().unwrap_or(0.6);
        if label.is_empty() {
            return None;
        }
        return Some(EmotionReport {
            label: label.to_ascii_lowercase(),
            intensity: intensity.clamp(0.0, 1.0),
        });
    }
    Some(EmotionReport {
        label: rest.to_ascii_lowercase(),
        intensity: 0.6,
    })
}

/// Discrete self-report consumed by the affect engine.
#[derive(Debug, Clone, PartialEq)]
pub struct EmotionReport {
    pub label: String,
    pub intensity: f32,
}

/// When the turn produced no thought, derive one from provider thinking.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_tags_from_speech() {
        let (speech, inner) = split_surface_and_inner(
            r#"hello <inner aspect="thought">ponder</inner> there <inner aspect="emotion">emotion: happy(0.9)</inner>"#,
        );
        assert_eq!(speech, "hello there");
        assert_eq!(inner.len(), 2);
        assert_eq!(inner[0].0, InnerAspect::Thought);
        assert_eq!(inner[0].1, "ponder");
        let report = parse_emotion_report(&inner[1].1).expect("emotion");
        assert_eq!(report.label, "happy");
        assert!((report.intensity - 0.9).abs() < f32::EPSILON);
        assert!(!model_visible_for(InnerAspect::Emotion));
        assert!(model_visible_for(InnerAspect::Thought));
    }

    #[test]
    fn derive_only_when_thought_missing() {
        assert!(derive_thought_from_thinking(&[], Some("hmm"), true).is_some());
        assert!(
            derive_thought_from_thinking(&[(InnerAspect::Thought, "x".into())], Some("hmm"), true)
                .is_none()
        );
    }
}
