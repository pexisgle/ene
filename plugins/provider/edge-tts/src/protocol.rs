//! Wire-frame parsing for the Edge speech WebSocket protocol.

pub struct BinaryFrame<'a> {
    pub path: &'a str,
    pub payload: &'a [u8],
}

pub fn parse_binary_frame(bytes: &[u8]) -> Result<BinaryFrame<'_>, String> {
    if bytes.len() < 2 {
        return Err("binary frame is shorter than the header length field".to_owned());
    }
    let header_len = usize::from(u16::from_be_bytes([bytes[0], bytes[1]]));
    let Some(header_end) = 2_usize.checked_add(header_len) else {
        return Err("header length overflows".to_owned());
    };
    if header_end > bytes.len() {
        return Err("header length exceeds frame size".to_owned());
    }
    let header = std::str::from_utf8(&bytes[2..header_end]).map_err(|err| err.to_string())?;
    let path =
        header_value(header, "Path").ok_or_else(|| "binary frame has no Path header".to_owned())?;
    Ok(BinaryFrame {
        path,
        payload: &bytes[header_end..],
    })
}

#[must_use]
pub fn text_path(text: &str) -> Option<&str> {
    let header = text.split_once("\r\n\r\n").map_or(text, |(head, _)| head);
    header_value(header, "Path")
}

fn header_value<'a>(header: &'a str, key: &str) -> Option<&'a str> {
    header.split("\r\n").find_map(|line| {
        let value = line.strip_prefix(key)?.strip_prefix(':')?;
        Some(value.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pcm_binary_frame() {
        let header = "X-RequestId:abc\r\nContent-Type:audio/x-wav\r\nPath:audio\r\n";
        let mut frame = Vec::new();
        frame.extend_from_slice(&u16::try_from(header.len()).unwrap().to_be_bytes());
        frame.extend_from_slice(header.as_bytes());
        frame.extend_from_slice(&[1, 2, 3]);
        let parsed = parse_binary_frame(&frame).unwrap();
        assert_eq!(parsed.path, "audio");
        assert_eq!(parsed.payload, &[1, 2, 3]);
    }

    #[test]
    fn extracts_text_path() {
        assert_eq!(
            text_path("X-RequestId:abc\r\nPath:turn.end\r\n\r\n{}"),
            Some("turn.end")
        );
    }
}
