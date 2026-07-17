//! Streaming-text token splitting, chunk-safe boundary handling, and
//! `<|perf:…|>` performance-marker parsing (#128).
//!
//! Markers are authoritative directives emitted mid-utterance by the LLM.
//! Unknown keys and invalid values soft-fail (log + drop) without aborting
//! the turn.

use crate::output::{MotionLayer, PerformanceCue};

/// Splits streaming text into normal text deltas and special tokens (like `<|perf:expr=happy|>`).
/// Handles partial tokens that span across chunks via a carry buffer.
pub fn split_text_and_special_tokens(
    carry: &mut String,
    chunk: &str,
) -> (Vec<String>, Vec<String>) {
    let mut buffer = std::mem::take(carry);
    buffer.push_str(chunk);

    let mut text_deltas = Vec::new();
    let mut special_tokens = Vec::new();
    let mut cursor = 0usize;

    while cursor < buffer.len() {
        match buffer[cursor..].find("<|") {
            None => {
                let remaining = &buffer[cursor..];
                if remaining.ends_with('<') {
                    let safe_end = buffer.len() - 1;
                    if cursor < safe_end {
                        text_deltas.push(buffer[cursor..safe_end].to_string());
                    }
                    *carry = "<".to_string();
                } else {
                    text_deltas.push(remaining.to_string());
                }
                return (text_deltas, special_tokens);
            }
            Some(open_rel) => {
                let open = cursor + open_rel;

                if open > cursor {
                    text_deltas.push(buffer[cursor..open].to_string());
                }

                let token_start = open + 2;

                match buffer[token_start..].find("|>") {
                    None => {
                        *carry = buffer[open..].to_string();
                        return (text_deltas, special_tokens);
                    }
                    Some(close_rel) => {
                        let close = token_start + close_rel;
                        let inner = &buffer[token_start..close];
                        special_tokens.push(format!("<|{inner}|>"));
                        cursor = close + 2;
                    }
                }
            }
        }
    }

    (text_deltas, special_tokens)
}

/// Extracts the emotion name from an emotion token like `<|emo:happy|>`,
/// returning `None` if the token is not a valid emotion token.
///
/// Deprecated in favour of [`parse_performance_marker`] for the `<|perf:…|>` grammar.
/// Kept for backward compatibility with LLMs that may still emit `<|emo:…|>` tokens.
pub fn extract_emotion_from_token(token: &str) -> Option<String> {
    let upper = token.to_ascii_uppercase();
    if !upper.starts_with("<|EMO:") || !upper.ends_with("|>") {
        return None;
    }

    let emotion = &token[6..token.len() - 2].trim();
    if emotion.is_empty() {
        return None;
    }
    Some(emotion.to_ascii_lowercase())
}

/// Strips performance markers from text and returns the cleaned string.
///
/// All `<|perf:…|>` and `<|emo:…|>` tokens are removed; plain text between
/// them is concatenated.
pub fn strip_markers(text: &str) -> String {
    let mut carry = String::new();
    let (text_deltas, _tokens) = split_text_and_special_tokens(&mut carry, text);
    if carry.is_empty() {
        text_deltas.concat()
    } else {
        text_deltas
            .into_iter()
            .chain(std::iter::once(carry))
            .collect()
    }
}

/// Parses a `<|perf:…|>` marker inner content into a [`PerformanceCue`].
///
/// Supported grammars:
/// ```text
/// <|perf:expr=NAME[,weight=FLOAT][,hold=SECS]|>
/// <|perf:motion=NAME[,layer=upper|lower|full]|>
/// <|perf:lookat=TARGET|>
/// <|perf:cancel=expr|motion|all|>
/// ```
///
/// Returns `None` for non-performance tokens (including `<|emo:…|>`, plain text, etc.).
/// Unknown keys are logged and silently dropped; the function still returns a best-effort
/// cue from the recognised keys.
pub fn parse_performance_marker(token: &str) -> Option<PerformanceCue> {
    let inner = strip_token_envelope(token)?;

    if inner.len() < 5 {
        return None;
    }

    let (kind, rest) = inner.split_once('=')?;
    let kind = kind.trim();

    match kind.to_ascii_lowercase().as_str() {
        "expr" => parse_expr_marker(rest),
        "motion" => parse_motion_marker(rest),
        "lookat" => Some(PerformanceCue::look_at(rest.trim())),
        "cancel" => Some(PerformanceCue::cancel(rest.trim())),
        other => {
            tracing::debug!(
                component = "PerfMarkerParser",
                kind = %other,
                "Unknown performance marker kind; ignoring"
            );
            None
        }
    }
}

fn strip_token_envelope(token: &str) -> Option<&str> {
    let t = token.trim();
    let upper = t.to_ascii_uppercase();
    const PREFIX: &str = "<|PERF:";
    if !upper.starts_with(PREFIX) || !t.ends_with("|>") {
        return None;
    }
    let inner = &t[PREFIX.len()..t.len() - 2];
    Some(inner.trim())
}

fn parse_expr_marker(rest: &str) -> Option<PerformanceCue> {
    let mut name = String::new();
    let mut weight: Option<f32> = None;
    let mut hold_secs: Option<f64> = None;

    for (i, part) in rest.split(',').enumerate() {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if i == 0 && !part.contains('=') {
            name = part.to_string();
            continue;
        }
        let Some((key, val)) = part.split_once('=') else {
            if i == 0 {
                name = part.to_string();
            } else {
                tracing::debug!(
                    component = "PerfMarkerParser",
                    part = %part,
                    "Ignoring unparseable expr key-value"
                );
            }
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "name" => name = val.trim().to_string(),
            "weight" => match val.trim().parse::<f32>() {
                Ok(w) if (0.0..=1.0).contains(&w) => weight = Some(w),
                Ok(w) => {
                    tracing::debug!(
                        component = "PerfMarkerParser",
                        weight = w,
                        "expr weight out of range [0,1]; clamping"
                    );
                    weight = Some(w.clamp(0.0, 1.0));
                }
                Err(_) => {
                    tracing::debug!(
                        component = "PerfMarkerParser",
                        val = %val,
                        "Invalid expr weight; ignoring"
                    );
                }
            },
            "hold" => match val.trim().parse::<f64>() {
                Ok(h) if h > 0.0 => hold_secs = Some(h),
                Ok(h) => {
                    tracing::debug!(
                        component = "PerfMarkerParser",
                        hold = h,
                        "expr hold must be positive; ignoring"
                    );
                }
                Err(_) => {
                    tracing::debug!(
                        component = "PerfMarkerParser",
                        val = %val,
                        "Invalid expr hold; ignoring"
                    );
                }
            },
            other => {
                tracing::debug!(
                    component = "PerfMarkerParser",
                    key = %other,
                    "Unknown expr key; ignoring"
                );
            }
        }
    }

    if name.is_empty() {
        tracing::debug!(
            component = "PerfMarkerParser",
            "expr marker requires a name"
        );
        return None;
    }

    if weight.is_some() || hold_secs.is_some() {
        Some(PerformanceCue::expression_with(
            name,
            weight.unwrap_or(1.0),
            hold_secs.unwrap_or(4.0),
        ))
    } else {
        Some(PerformanceCue::expression(name))
    }
}

fn parse_motion_marker(rest: &str) -> Option<PerformanceCue> {
    let mut name = String::new();
    let mut layer: Option<MotionLayer> = None;

    for (i, part) in rest.split(',').enumerate() {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if i == 0 && !part.contains('=') {
            name = part.to_string();
            continue;
        }
        let Some((key, val)) = part.split_once('=') else {
            if i == 0 {
                name = part.to_string();
            }
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "name" => name = val.trim().to_string(),
            "layer" => {
                layer = match val.trim().to_ascii_lowercase().as_str() {
                    "upper" => Some(MotionLayer::Upper),
                    "lower" => Some(MotionLayer::Lower),
                    "full" => Some(MotionLayer::Full),
                    other => {
                        tracing::debug!(
                            component = "PerfMarkerParser",
                            layer = %other,
                            "Unknown motion layer; ignoring"
                        );
                        None
                    }
                };
            }
            other => {
                tracing::debug!(
                    component = "PerfMarkerParser",
                    key = %other,
                    "Unknown motion key; ignoring"
                );
            }
        }
    }

    if name.is_empty() {
        tracing::debug!(
            component = "PerfMarkerParser",
            "motion marker requires a name"
        );
        return None;
    }

    Some(PerformanceCue::motion(name, layer))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── extract_emotion_from_token (backward compat) ──

    #[test]
    fn test_extract_emotion_simple() {
        assert_eq!(
            extract_emotion_from_token("<|emo:happy|>"),
            Some("happy".to_string())
        );
        assert_eq!(
            extract_emotion_from_token("<|EMO:HAPPY|>"),
            Some("happy".to_string())
        );
        assert_eq!(
            extract_emotion_from_token("<|emo:Sad|>"),
            Some("sad".to_string())
        );
    }

    #[test]
    fn test_extract_emotion_invalid() {
        assert_eq!(extract_emotion_from_token("<|ACT:happy|>"), None);
        assert_eq!(extract_emotion_from_token("<|emo:|>"), None);
        assert_eq!(extract_emotion_from_token("not a token"), None);
    }

    #[test]
    fn test_extract_emotion_whitespace() {
        assert_eq!(
            extract_emotion_from_token("<|EMO:  happy  |>"),
            Some("happy".to_string())
        );
    }

    // ── split_text_and_special_tokens ──

    #[test]
    fn test_split_text_and_special_tokens_no_tokens() {
        let mut carry = String::new();
        let (text, tokens) = split_text_and_special_tokens(&mut carry, "Hello world");
        assert_eq!(text, vec!["Hello world"]);
        assert!(tokens.is_empty());
        assert!(carry.is_empty());
    }

    #[test]
    fn test_split_text_and_special_tokens_with_emotion() {
        let mut carry = String::new();
        let (text, tokens) = split_text_and_special_tokens(&mut carry, "Hello <|emo:happy|> world");
        assert_eq!(text, vec!["Hello ", " world"]);
        assert_eq!(tokens, vec!["<|emo:happy|>"]);
        assert!(carry.is_empty());
    }

    #[test]
    fn test_split_text_and_special_tokens_with_perf() {
        let mut carry = String::new();
        let (text, tokens) =
            split_text_and_special_tokens(&mut carry, "Hi <|perf:expr=happy|> there");
        assert_eq!(text, vec!["Hi ", " there"]);
        assert_eq!(tokens, vec!["<|perf:expr=happy|>"]);
        assert!(carry.is_empty());
    }

    #[test]
    fn test_split_text_and_special_tokens_multiple() {
        let mut carry = String::new();
        let (text, tokens) =
            split_text_and_special_tokens(&mut carry, "A <|emo:happy|> B <|emo:sad|> C");
        assert_eq!(text, vec!["A ", " B ", " C"]);
        assert_eq!(tokens, vec!["<|emo:happy|>", "<|emo:sad|>"]);
        assert!(carry.is_empty());
    }

    #[test]
    fn test_split_text_and_special_tokens_incomplete_at_end() {
        let mut carry = String::new();
        let (text, tokens) = split_text_and_special_tokens(&mut carry, "Hello <|emo");
        assert_eq!(text, vec!["Hello "]);
        assert!(tokens.is_empty());
        assert_eq!(carry, "<|emo");
    }

    #[test]
    fn test_split_text_and_special_tokens_carry_continuation() {
        let mut carry = String::new();
        let (text1, _tokens1) = split_text_and_special_tokens(&mut carry, "Hello <|emo");
        assert_eq!(text1, vec!["Hello "]);
        assert_eq!(carry, "<|emo");

        let (text2, tokens2) = split_text_and_special_tokens(&mut carry, ":happy|> world");
        assert_eq!(text2, vec![" world"]);
        assert_eq!(tokens2, vec!["<|emo:happy|>"]);
        assert!(carry.is_empty());
    }

    #[test]
    fn test_split_text_and_special_tokens_trailing_angle_bracket() {
        let mut carry = String::new();
        let (text, tokens) = split_text_and_special_tokens(&mut carry, "Hello <");
        assert_eq!(text, vec!["Hello "]);
        assert!(tokens.is_empty());
        assert_eq!(carry, "<");
    }

    #[test]
    fn test_split_text_and_special_tokens_empty_chunk() {
        let mut carry = String::new();
        let (text, tokens) = split_text_and_special_tokens(&mut carry, "");
        assert!(text.is_empty());
        assert!(tokens.is_empty());
        assert!(carry.is_empty());
    }

    // ── strip_markers ──

    #[test]
    fn strip_markers_removes_emo_and_perf() {
        let input = "Hello <|emo:happy|> <|perf:motion=wave|> world";
        assert_eq!(strip_markers(input), "Hello   world");
    }

    #[test]
    fn strip_markers_no_markers() {
        assert_eq!(strip_markers("plain text"), "plain text");
    }

    // ── parse_performance_marker ──

    #[test]
    fn parse_expr_simple() {
        let cue = parse_performance_marker("<|perf:expr=happy|>").unwrap();
        assert_eq!(cue.kind, crate::output::PerfKind::Expression);
        assert_eq!(cue.name, "happy");
        assert!(cue.weight.is_none());
    }

    #[test]
    fn parse_expr_with_weight() {
        let cue = parse_performance_marker("<|perf:expr=happy,weight=0.8,hold=2.0|>").unwrap();
        assert_eq!(cue.name, "happy");
        assert_eq!(cue.weight, Some(0.8));
        assert_eq!(cue.hold_secs, Some(2.0));
    }

    #[test]
    fn parse_expr_weight_clamped() {
        let cue = parse_performance_marker("<|perf:expr=happy,weight=1.5|>").unwrap();
        assert_eq!(cue.weight, Some(1.0));
    }

    #[test]
    fn parse_expr_negative_hold_ignored() {
        let cue = parse_performance_marker("<|perf:expr=happy,hold=-1.0|>").unwrap();
        assert_eq!(cue.hold_secs, None);
    }

    #[test]
    fn parse_expr_case_insensitive_keys() {
        let cue = parse_performance_marker("<|perf:expr=happy,WEIGHT=0.5,HOLD=3.0|>").unwrap();
        assert_eq!(cue.name, "happy");
        assert_eq!(cue.weight, Some(0.5));
        assert_eq!(cue.hold_secs, Some(3.0));
    }

    #[test]
    fn parse_expr_name_first_positional() {
        let cue = parse_performance_marker("<|perf:expr=happy|>").unwrap();
        assert_eq!(cue.name, "happy");
    }

    #[test]
    fn parse_expr_name_keyed() {
        let cue = parse_performance_marker("<|perf:expr=name=happy,weight=0.5|>").unwrap();
        assert_eq!(cue.name, "happy");
        assert_eq!(cue.weight, Some(0.5));
    }

    #[test]
    fn parse_expr_empty_name_fails() {
        assert!(parse_performance_marker("<|perf:expr=,weight=1.0|>").is_none());
    }

    #[test]
    fn parse_motion_simple() {
        let cue = parse_performance_marker("<|perf:motion=wave|>").unwrap();
        assert_eq!(cue.kind, crate::output::PerfKind::Motion);
        assert_eq!(cue.name, "wave");
        assert!(cue.motion_layer.is_none());
    }

    #[test]
    fn parse_motion_with_layer() {
        let cue = parse_performance_marker("<|perf:motion=wave,layer=upper|>").unwrap();
        assert_eq!(cue.name, "wave");
        assert_eq!(cue.motion_layer, Some(MotionLayer::Upper));
    }

    #[test]
    fn parse_motion_full_layer() {
        let cue = parse_performance_marker("<|perf:motion=wave,layer=full|>").unwrap();
        assert_eq!(cue.motion_layer, Some(MotionLayer::Full));
    }

    #[test]
    fn parse_motion_unknown_layer_soft_fails() {
        let cue = parse_performance_marker("<|perf:motion=wave,layer=torso|>").unwrap();
        assert_eq!(cue.name, "wave");
        assert!(cue.motion_layer.is_none());
    }

    #[test]
    fn parse_lookat() {
        let cue = parse_performance_marker("<|perf:lookat=user|>").unwrap();
        assert_eq!(cue.kind, crate::output::PerfKind::LookAt);
        assert_eq!(cue.name, "user");
    }

    #[test]
    fn parse_cancel() {
        let cue = parse_performance_marker("<|perf:cancel=expr|>").unwrap();
        assert_eq!(cue.kind, crate::output::PerfKind::Cancel);
        assert_eq!(cue.name, "expr");
    }

    #[test]
    fn parse_non_perf_token_returns_none() {
        assert!(parse_performance_marker("<|emo:happy|>").is_none());
        assert!(parse_performance_marker("plain text").is_none());
        assert!(parse_performance_marker("<|ACT:do|>").is_none());
    }

    #[test]
    fn parse_unknown_kind_soft_fails() {
        assert!(parse_performance_marker("<|perf:bogus=xyz|>").is_none());
    }

    #[test]
    fn parse_whitespace_handling() {
        let cue = parse_performance_marker("<|perf:  expr = happy , weight = 0.5 |>").unwrap();
        assert_eq!(cue.name, "happy");
        assert_eq!(cue.weight, Some(0.5));
    }

    #[test]
    fn parse_unknown_keys_soft_failed_but_name_kept() {
        let cue = parse_performance_marker("<|perf:motion=wave,bogus=ignored|>").unwrap();
        assert_eq!(cue.name, "wave");
    }

    #[test]
    fn parse_expr_with_unknown_key_still_valid() {
        let cue = parse_performance_marker("<|perf:expr=sad,intensity=high|>").unwrap();
        assert_eq!(cue.name, "sad");
    }

    #[test]
    fn parse_invalid_weight_ignored() {
        let cue = parse_performance_marker("<|perf:expr=happy,weight=abc|>").unwrap();
        assert_eq!(cue.name, "happy");
        assert!(cue.weight.is_none());
    }
}
