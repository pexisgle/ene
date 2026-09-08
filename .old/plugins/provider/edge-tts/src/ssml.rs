const MAX_CHUNK_BYTES: usize = 4096;

#[must_use]
pub fn normalize_voice(voice: &str) -> String {
    let Some((lang, rest)) = voice.split_once('-') else {
        return voice.to_owned();
    };
    if lang.len() < 2 || !lang.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return voice.to_owned();
    }
    let Some((region, name)) = rest.split_once('-') else {
        return voice.to_owned();
    };
    if region.len() < 2 || !region.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return voice.to_owned();
    }
    let (region, name) = match name.split_once('-') {
        Some((extra, tail)) => (format!("{region}-{extra}"), tail.to_owned()),
        None => (region.to_owned(), name.to_owned()),
    };
    format!("Microsoft Server Speech Text to Speech Voice ({lang}-{region}, {name})")
}

#[must_use]
pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|ch| {
            let code = u32::from(ch);
            if code <= 8 || (11..=12).contains(&code) || (14..=31).contains(&code) {
                ' '
            } else {
                ch
            }
        })
        .collect()
}

#[must_use]
pub fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[must_use]
pub fn chunk_text(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut rest = text;
    while rest.len() > MAX_CHUNK_BYTES {
        let mut split = MAX_CHUNK_BYTES;
        while !rest.is_char_boundary(split) {
            split = split.saturating_sub(1);
        }
        split = rest[..split]
            .rfind('\n')
            .or_else(|| rest[..split].rfind(' '))
            .map_or(split, |index| index.saturating_add(1));
        if let Some(amp) = rest[..split].rfind('&')
            && !rest[amp..split].contains(';')
        {
            split = amp;
        }
        let chunk = rest[..split].trim();
        if !chunk.is_empty() {
            chunks.push(chunk.to_owned());
        }
        rest = rest.get(split.max(1)..).unwrap_or("");
    }
    let tail = rest.trim();
    if !tail.is_empty() {
        chunks.push(tail.to_owned());
    }
    chunks
}

#[must_use]
pub fn build_ssml(voice: &str, locale: &str, text: &str) -> String {
    let voice = normalize_voice(voice);
    format!(
        "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='{locale}'><voice name='{voice}'><prosody pitch='+0Hz' rate='+0%' volume='+0%'>{text}</prosody></voice></speak>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_short_voice_names() {
        assert_eq!(
            normalize_voice("ja-JP-NanamiNeural"),
            "Microsoft Server Speech Text to Speech Voice (ja-JP, NanamiNeural)"
        );
    }

    #[test]
    fn chunks_short_text() {
        assert_eq!(chunk_text("hello"), vec!["hello".to_owned()]);
    }
}
