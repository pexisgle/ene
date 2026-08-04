//! SSML construction: voice normalization, text sanitization, and
//! byte-limited chunking, mirroring the python edge-tts client.

use crate::config::EdgeTtsConfig;

/// Per-request cap on the escaped SSML text body; the service rejects
/// longer payloads.
pub const MAX_CHUNK_BYTES: usize = 4096;

/// Converts a short Edge voice name (`ja-JP-NanamiNeural`) to the long
/// `Microsoft Server Speech Text to Speech Voice (ja-JP, NanamiNeural)`
/// form the Edge client sends; anything that does not match passes through
/// unchanged.
#[must_use]
pub fn normalize_voice(voice: &str) -> String {
    let Some((lang, rest)) = voice.split_once('-') else {
        return voice.to_string();
    };
    if lang.len() < 2 || !lang.bytes().all(|b| b.is_ascii_lowercase()) {
        return voice.to_string();
    }
    let Some((region, name)) = rest.split_once('-') else {
        return voice.to_string();
    };
    if region.len() < 2 || !region.bytes().all(|b| b.is_ascii_uppercase()) {
        return voice.to_string();
    }
    let (region, name) = match name.split_once('-') {
        Some((extra, tail)) => (format!("{region}-{extra}"), tail.to_string()),
        None => (region.to_string(), name.to_string()),
    };
    format!("Microsoft Server Speech Text to Speech Voice ({lang}-{region}, {name})")
}

/// Replaces control characters the service rejects (C0 controls except tab,
/// LF, and CR) with spaces, so OCR'd text with vertical tabs does not make
/// the request fail.
#[must_use]
pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            let code = u32::from(c);
            if code <= 8 || (11..=12).contains(&code) || (14..=31).contains(&code) {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// Escapes the three characters with XML markup meaning in text content.
#[must_use]
pub fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Splits escaped text into chunks of at most [`MAX_CHUNK_BYTES`] bytes,
/// preferring newline/space boundaries, never splitting a UTF-8 sequence or
/// an XML entity. Whitespace-only chunks are dropped.
#[must_use]
pub fn chunk_text(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut rest = text;
    while rest.len() > MAX_CHUNK_BYTES {
        let mut split = MAX_CHUNK_BYTES;
        while !rest.is_char_boundary(split) {
            split -= 1;
        }
        split = rest[..split]
            .rfind('\n')
            .or_else(|| rest[..split].rfind(' '))
            .map_or(split, |index| index + 1);
        if let Some(amp) = rest[..split].rfind('&')
            && !rest[amp..split].contains(';')
        {
            split = amp;
        }
        let chunk = rest[..split].trim();
        if !chunk.is_empty() {
            chunks.push(chunk.to_string());
        }
        rest = &rest[split.max(1)..];
    }
    let tail = rest.trim();
    if !tail.is_empty() {
        chunks.push(tail.to_string());
    }
    chunks
}

/// Builds the full SSML document for one text chunk. `text` must already be
/// sanitized, escaped, and within [`MAX_CHUNK_BYTES`].
#[must_use]
pub fn build_ssml(config: &EdgeTtsConfig, text: &str) -> String {
    let voice = normalize_voice(&config.voice);
    format!(
        "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='{}'><voice name='{voice}'><prosody pitch='{}' rate='{}' volume='{}'>{text}</prosody></voice></speak>",
        config.locale, config.pitch, config.rate, config.volume
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EdgeTtsConfig;

    #[test]
    fn normalizes_short_voice_names() {
        assert_eq!(
            normalize_voice("ja-JP-NanamiNeural"),
            "Microsoft Server Speech Text to Speech Voice (ja-JP, NanamiNeural)"
        );
        assert_eq!(
            normalize_voice("en-US-AvaMultilingualNeural"),
            "Microsoft Server Speech Text to Speech Voice (en-US, AvaMultilingualNeural)"
        );
        assert_eq!(
            normalize_voice("zh-CN-XiaoxiaoNeural"),
            "Microsoft Server Speech Text to Speech Voice (zh-CN, XiaoxiaoNeural)"
        );
        // The upstream client normalizes any three-part name, Neural or not.
        assert_eq!(
            normalize_voice("ja-JP-NotNeural"),
            "Microsoft Server Speech Text to Speech Voice (ja-JP, NotNeural)"
        );
        assert_eq!(
            normalize_voice("zh-CN-liaoning-XiaobeiNeural"),
            "Microsoft Server Speech Text to Speech Voice (zh-CN-liaoning, XiaobeiNeural)"
        );
    }

    #[test]
    fn passes_through_non_short_names() {
        let long = "Microsoft Server Speech Text to Speech Voice (ja-JP, NanamiNeural)";
        assert_eq!(normalize_voice(long), long);
        assert_eq!(normalize_voice("custom"), "custom");
        assert_eq!(normalize_voice("ja-jp-NanamiNeural"), "ja-jp-NanamiNeural");
        assert_eq!(normalize_voice("ja-JP"), "ja-JP");
    }

    #[test]
    fn sanitizes_rejected_control_characters() {
        assert_eq!(sanitize("a\u{000b}b\u{0007}c"), "a b c");
        assert_eq!(sanitize("a\tb\nc\rd"), "a\tb\nc\rd");
        assert_eq!(sanitize("日本語"), "日本語");
    }

    #[test]
    fn escapes_xml_special_characters() {
        assert_eq!(escape_xml("a & b < c > d"), "a &amp; b &lt; c &gt; d");
        assert_eq!(escape_xml("日本語"), "日本語");
    }

    #[test]
    fn chunks_within_byte_limit() {
        let text = "x".repeat(MAX_CHUNK_BYTES + 100);
        let chunks = chunk_text(&text);
        assert_eq!(chunks.len(), 2);
        assert!(chunks.iter().all(|c| c.len() <= MAX_CHUNK_BYTES));
        assert_eq!(chunks.concat().len(), text.len());
    }

    #[test]
    fn chunks_prefer_whitespace_boundaries() {
        let text = format!("{}\n{}", "a".repeat(MAX_CHUNK_BYTES - 10), "b".repeat(100));
        let chunks = chunk_text(&text);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].ends_with('a'));
        assert!(chunks[1].starts_with('b'));
    }

    #[test]
    fn chunks_never_split_utf8_or_entities() {
        let text = format!("{}{}", "日".repeat(1500), "&amp;".repeat(200));
        let chunks = chunk_text(&text);
        for chunk in &chunks {
            assert!(chunk.is_char_boundary(0));
            assert!(chunk.is_char_boundary(chunk.len()));
        }
        assert_eq!(chunks.concat().len(), text.len());
        assert!(chunks.concat().ends_with("&amp;"));
    }

    #[test]
    fn chunks_drop_whitespace_only_pieces() {
        let text = format!(
            "{}     \n{}",
            "a".repeat(MAX_CHUNK_BYTES - 5),
            "b".repeat(5)
        );
        let chunks = chunk_text(&text);
        assert!(chunks.iter().all(|c| !c.trim().is_empty()));
    }

    #[test]
    fn short_text_is_one_chunk() {
        assert_eq!(chunk_text("こんにちは"), vec!["こんにちは"]);
        assert!(chunk_text("   ").is_empty());
        assert!(chunk_text("").is_empty());
    }

    #[test]
    fn builds_ssml_with_configured_prosody_and_locale() {
        let config = EdgeTtsConfig {
            voice: "ja-JP-NanamiNeural".to_string(),
            locale: "ja-JP".to_string(),
            rate: "+10%".to_string(),
            pitch: "-5Hz".to_string(),
            volume: "+20%".to_string(),
            ..EdgeTtsConfig::default()
        };
        let ssml = build_ssml(&config, "こんにちは &amp; さようなら");
        assert!(ssml.starts_with(
            "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='ja-JP'>"
        ));
        assert!(ssml.contains(
            "<voice name='Microsoft Server Speech Text to Speech Voice (ja-JP, NanamiNeural)'>"
        ));
        assert!(ssml.contains("<prosody pitch='-5Hz' rate='+10%' volume='+20%'>"));
        assert!(ssml.contains("こんにちは &amp; さようなら"));
        assert!(ssml.ends_with("</prosody></voice></speak>"));
    }
}
