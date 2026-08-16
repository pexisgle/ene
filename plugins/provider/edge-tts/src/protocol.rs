//! Wire-frame parsing for the Edge speech WebSocket protocol.
//!
//! Text frames carry a CRLF header block (`Key:value` lines, no space after
//! the colon) followed by a JSON body. Binary frames start with a 2-byte
//! big-endian header length, then the same header block, then the raw audio
//! payload.

use crate::error::EdgeError;

#[derive(Debug)]
pub struct BinaryFrame<'a> {
    /// Value of the `Path` header line.
    pub path: &'a str,
    /// Value of the `Content-Type` header line, when present.
    pub content_type: Option<&'a str>,
    /// Audio bytes following the header block.
    pub payload: &'a [u8],
}

/// Parses a binary frame into its header block and payload.
///
/// # Errors
///
/// Returns [`EdgeError::Protocol`] when the frame is shorter than the header
/// length field, the header length overruns the frame, or the header is not
/// UTF-8.
pub fn parse_binary_frame(bytes: &[u8]) -> Result<BinaryFrame<'_>, EdgeError> {
    if bytes.len() < 2 {
        return Err(EdgeError::Protocol(
            "binary frame is shorter than the header length field".to_string(),
        ));
    }
    let header_len = usize::from(u16::from_be_bytes([bytes[0], bytes[1]]));
    let Some(header_end) = 2usize.checked_add(header_len) else {
        return Err(EdgeError::Protocol("header length overflows".to_string()));
    };
    if header_end > bytes.len() {
        return Err(EdgeError::Protocol(format!(
            "header length {header_len} exceeds frame size {}",
            bytes.len()
        )));
    }
    let header = std::str::from_utf8(&bytes[2..header_end])
        .map_err(|e| EdgeError::Protocol(format!("header is not UTF-8: {e}")))?;
    let path = header_value(header, "Path")
        .ok_or_else(|| EdgeError::Protocol("binary frame has no Path header".to_string()))?;
    let content_type = header_value(header, "Content-Type");
    Ok(BinaryFrame {
        path,
        content_type,
        payload: &bytes[header_end..],
    })
}

#[must_use]
pub fn text_path(text: &str) -> Option<&str> {
    let header = text.split_once("\r\n\r\n").map_or(text, |(h, _)| h);
    header_value(header, "Path")
}

fn header_value<'a>(header: &'a str, key: &str) -> Option<&'a str> {
    header.split("\r\n").find_map(|line| {
        let value = line.strip_prefix(key)?.strip_prefix(':')?;
        Some(value.trim())
    })
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use super::*;

    // The service counts the trailing CRLF of the header block inside the
    // length field; the MP3 payload starts directly after it.
    const AUDIO_HEADER: &str = "X-RequestId:abc\r\nContent-Type:audio/mpeg\r\nPath:audio\r\n";

    fn binary_frame(header: &str, payload: &[u8]) -> Vec<u8> {
        let mut frame = Vec::new();
        frame.extend_from_slice(&(header.len() as u16).to_be_bytes());
        frame.extend_from_slice(header.as_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    #[test]
    fn parses_audio_binary_frame() {
        let frame = binary_frame(AUDIO_HEADER, &[0x49, 0x44, 0x33]);
        let parsed = parse_binary_frame(&frame).expect("valid frame");
        assert_eq!(parsed.path, "audio");
        assert_eq!(parsed.content_type, Some("audio/mpeg"));
        assert_eq!(parsed.payload, &[0x49, 0x44, 0x33]);
    }

    #[test]
    fn parses_terminal_frame_without_content_type() {
        let frame = binary_frame("X-RequestId:abc\r\nPath:audio\r\n", &[]);
        let parsed = parse_binary_frame(&frame).expect("valid frame");
        assert_eq!(parsed.path, "audio");
        assert_eq!(parsed.content_type, None);
        assert!(parsed.payload.is_empty());
    }

    #[test]
    fn rejects_frames_shorter_than_header_length_field() {
        let err = parse_binary_frame(&[0x00]).expect_err("too short");
        assert!(matches!(err, EdgeError::Protocol(_)));
    }

    #[test]
    fn rejects_header_length_overrun() {
        let err = parse_binary_frame(&[0x30, 0x00, b'a']).expect_err("overrun");
        assert!(matches!(err, EdgeError::Protocol(_)));
    }

    #[test]
    fn rejects_missing_path_header() {
        let frame = binary_frame("Content-Type:audio/mpeg\r\n", &[]);
        let err = parse_binary_frame(&frame).expect_err("no Path");
        assert!(matches!(err, EdgeError::Protocol(_)));
    }

    #[test]
    fn extracts_text_path() {
        assert_eq!(
            text_path("X-RequestId:abc\r\nPath:turn.end\r\n\r\n{}"),
            Some("turn.end")
        );
        assert_eq!(
            text_path("Path:speech.config\r\n\r\n{}"),
            Some("speech.config")
        );
        assert_eq!(text_path("no header here"), None);
    }
}
