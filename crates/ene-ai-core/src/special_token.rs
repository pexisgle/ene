pub fn split_text_and_special_tokens(carry: &mut String, chunk: &str) -> (Vec<String>, Vec<String>) {
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

pub fn extract_emotion_from_act_token(token: &str) -> Option<String> {
    let upper = token.to_ascii_uppercase();
    if !upper.starts_with("<|ACT:") || !upper.ends_with("|>") {
        return None;
    }
    
    let payload = &token[6..token.len() - 2].trim();

    // Try JSON first: {"emotion": "happy"}
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) {
        if let Some(emotion) = v.get("emotion").and_then(|e| e.as_str()) {
            return Some(emotion.to_ascii_lowercase());
        }
    }

    // Fallback: keyword scan of the raw payload
    let lower = payload.to_ascii_lowercase();
    for keyword in ["happy", "joy", "sad", "angry", "relaxed", "surprised", "neutral"] {
        if lower.contains(keyword) {
            return Some(keyword.to_string());
        }
    }
    None
}
