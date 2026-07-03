use super::input::RecallTurn;

const MAX_TOPIC_CHARS: usize = 240;
const MAX_SCENE_CHARS: usize = 160;

pub(crate) fn current_topic(
    user_input: &str,
    recent_turns: &[RecallTurn<'_>],
    scene_summary: Option<&str>,
) -> Option<String> {
    let scene = normalize_text(scene_summary.unwrap_or_default());
    let base = normalize_text(user_input)
        .or_else(|| recent_user_turn(recent_turns))
        .or_else(|| scene.clone())?;

    match scene {
        Some(scene) if !contains_case_insensitive(&base, &scene) => Some(format!(
            "{base} | scene: {}",
            truncate_chars(&scene, MAX_SCENE_CHARS)
        )),
        _ => Some(base),
    }
}

pub(crate) fn normalize_text(text: &str) -> Option<String> {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(truncate_chars(&normalized, MAX_TOPIC_CHARS))
    }
}

pub(crate) fn recent_user_turn(recent_turns: &[RecallTurn<'_>]) -> Option<String> {
    recent_turns
        .iter()
        .rev()
        .find(|turn| turn.role.eq_ignore_ascii_case("user"))
        .and_then(|turn| normalize_text(turn.content))
}

fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in text.chars().take(max_chars) {
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_summary_alone_becomes_topic() {
        let topic = current_topic("", &[], Some("A quiet evening at the cafe"))
            .expect("scene summary should be usable as topic");

        assert_eq!(topic, "A quiet evening at the cafe");
    }

    #[test]
    fn user_input_with_distinct_scene_appends_scene_suffix() {
        let topic =
            current_topic("Tell me more", &[], Some("A quiet evening at the cafe")).expect("topic");

        assert_eq!(topic, "Tell me more | scene: A quiet evening at the cafe");
    }

    #[test]
    fn whitespace_only_input_without_context_returns_none() {
        assert!(current_topic("   ", &[], None).is_none());
    }
}
