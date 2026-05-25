/// Splits streaming text into normal text deltas and special tokens (like `<|emo:happy|>`).
/// Handles partial tokens that span across chunks via a carry buffer.
pub fn split_text_and_special_tokens(
    carry: &mut String,
    chunk: &str,
) -> (Vec<String>, Vec<String>) {
    // Prepend anything carried over from the previous chunk.
    let mut buffer = std::mem::take(carry);
    buffer.push_str(chunk);

    let mut text_deltas = Vec::new();
    let mut special_tokens = Vec::new();
    let mut cursor = 0usize;

    while cursor < buffer.len() {
        match buffer[cursor..].find("<|") {
            None => {
                // No opening `<|` in the remaining buffer.
                // If the buffer ends with a bare `<`, it *might* be the start of a `<|`
                // that will complete in the next chunk — carry it over.
                let remaining = &buffer[cursor..];
                if remaining.ends_with('<') {
                    let safe_end = buffer.len() - 1;
                    if cursor < safe_end {
                        text_deltas.push(buffer[cursor..safe_end].to_string());
                    }
                    // Carry the trailing `<` — do NOT clear carry at the end.
                    *carry = "<".to_string();
                } else {
                    text_deltas.push(remaining.to_string());
                    // carry is already empty (taken above); nothing to restore.
                }
                return (text_deltas, special_tokens);
            }
            Some(open_rel) => {
                let open = cursor + open_rel;

                // Emit any text before the `<|`.
                if open > cursor {
                    text_deltas.push(buffer[cursor..open].to_string());
                }

                let token_start = open + 2; // skip `<|`

                match buffer[token_start..].find("|>") {
                    None => {
                        // Closing `|>` has not arrived yet — carry the incomplete token.
                        *carry = buffer[open..].to_string();
                        return (text_deltas, special_tokens);
                    }
                    Some(close_rel) => {
                        let close = token_start + close_rel;
                        let inner = &buffer[token_start..close];
                        special_tokens.push(format!("<|{inner}|>"));
                        cursor = close + 2; // skip `|>`
                    }
                }
            }
        }
    }

    // carry is already empty (taken above); nothing left to restore.
    (text_deltas, special_tokens)
}

/// Extracts the emotion name from an emotion token like `<|emo:happy|>`,
/// returning `None` if the token is not a valid emotion token.
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

#[cfg(test)]
mod tests {
    use super::*;

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
        // First chunk: incomplete token
        let (text1, _tokens1) = split_text_and_special_tokens(&mut carry, "Hello <|emo");
        assert_eq!(text1, vec!["Hello "]);
        assert_eq!(carry, "<|emo");

        // Second chunk: completes the token
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
}
