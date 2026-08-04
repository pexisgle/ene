//! `ElevenLabs` WebSocket client for the `stream-input` endpoint:
//! bidirectional text/audio streaming, base64 audio collection, and
//! whole-request retries.
//!
//! The server is stateful per connection (settings are applied by the init
//! frame and text is buffered until a chunk is ready), so a mid-stream
//! failure cannot be resumed without duplicating audio: the entire request
//! restarts from scratch and partial audio is discarded.
//!
use std::time::Duration;

use base64::Engine;
use futures::{SinkExt, StreamExt};
use http::header::HeaderValue;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use unicode_segmentation::UnicodeSegmentation;

use ene_plugin::{PluginError, ProviderErrorKind};

use crate::client::{
    CONNECT_TIMEOUT, MAX_ATTEMPTS, MAX_PCM_BYTES, REQUEST_TIMEOUT, is_transient_status,
    map_http_error, mask_key, retry_after_secs, retry_delay,
};
use crate::config::ElevenLabsConfig;
use crate::pcm;
use crate::wav;

/// Idle ceiling between server messages; a long utterance keeps audio
/// frames flowing well inside this window.
const MESSAGE_TIMEOUT: Duration = Duration::from_mins(1);
/// Largest text frame; the server re-chunks by its schedule anyway, but
/// small frames keep the first audio chunk's latency low.
const MAX_TEXT_CHUNK_CHARS: usize = 512;
/// Sentence boundary characters for text chunking.
const SENTENCE_BOUNDARIES: &[char] = &['。', '．', '！', '？', '.', '!', '?', '\n'];
/// Characters percent-encoded in query components; the unreserved set
/// (`A-Za-z0-9-._~`) is left alone.
const QUERY_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'+')
    .add(b'/')
    .add(b':')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// Synthesizes `text` over the `stream-input` WebSocket and returns WAV
/// bytes. The whole request is retried from scratch on retryable failures.
///
/// # Errors
///
/// Returns a provider error for server-reported errors (401/403 → `Auth`,
/// 429 → `RateLimit`, retried like the REST path), malformed audio frames,
/// and transport failures that exhaust the retry budget. A typed
/// `Truncated` error surfaces when the collected PCM ends mid-sample.
pub async fn synthesize_ws(
    config: &ElevenLabsConfig,
    api_key: &str,
    base_url: &str,
    text: &str,
    voice_id: &str,
) -> Result<Vec<u8>, PluginError> {
    let url = ws_url(base_url, voice_id, config);
    let mut attempt: u32 = 0;
    loop {
        match synthesize_ws_pass(&url, api_key, config, text).await {
            Ok(audio) => {
                pcm::validate_pcm(&audio)?;
                return wav::wrap_pcm(&audio, config.sample_rate);
            }
            Err(failure) => {
                let next = attempt.saturating_add(1);
                if !failure.is_retryable() || next >= MAX_ATTEMPTS {
                    return Err(failure.into_plugin_error());
                }
                let delay = retry_delay(failure.retry_after(), attempt);
                tracing::warn!(
                    component = "ene-plugin-elevenlabs",
                    attempt = next,
                    delay_ms = delay.as_millis() as u64,
                    error = mask_key(&failure.message(), api_key),
                    "elevenlabs websocket stream failed; retrying"
                );
                tokio::time::sleep(delay).await;
                attempt = next;
            }
        }
    }
}

/// Runs one connection pass: handshake, init frame, text chunks, terminal
/// frame, then audio collection until the server's final frame.
async fn synthesize_ws_pass(
    url: &str,
    api_key: &str,
    config: &ElevenLabsConfig,
    text: &str,
) -> Result<Vec<u8>, WsFailure> {
    let mut ws = connect(url, api_key).await?;
    // A single-space init frame applies voice settings without producing
    // audio; later text frames carry the utterance.
    let init = json!({
        "text": " ",
        "voice_settings": config.voice_settings,
        "generation_config": { "chunk_length_schedule": [120, 160, 250, 290] }
    });
    send(&mut ws, &init.to_string()).await?;
    for chunk in split_text(text, MAX_TEXT_CHUNK_CHARS) {
        send(&mut ws, &json!({ "text": chunk }).to_string()).await?;
    }
    send(&mut ws, &json!({ "text": "" }).to_string()).await?;

    let audio = match timeout(REQUEST_TIMEOUT, collect_audio(&mut ws, api_key)).await {
        Err(_) => {
            return Err(WsFailure::retryable(
                "elevenlabs websocket timed out before the final frame",
                None,
            ));
        }
        Ok(result) => result?,
    };
    if audio.is_empty() {
        return Err(WsFailure::terminal(PluginError::provider(
            "elevenlabs returned an empty audio response",
        )));
    }
    Ok(audio)
}

/// Reads server frames until `isFinal`, appending decoded audio chunks.
async fn collect_audio(
    ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    api_key: &str,
) -> Result<Vec<u8>, WsFailure> {
    let mut audio = Vec::new();
    loop {
        let message = match timeout(MESSAGE_TIMEOUT, ws.next()).await {
            Err(_) => {
                return Err(WsFailure::retryable(
                    "elevenlabs websocket timed out waiting for an audio message",
                    None,
                ));
            }
            Ok(Some(Ok(Message::Text(text)))) => text,
            Ok(Some(Ok(Message::Close(_))) | None) => {
                return Err(WsFailure::retryable(
                    "elevenlabs websocket closed before the final frame",
                    None,
                ));
            }
            // Binary/ping/pong frames carry nothing usable; pings are
            // answered automatically by the stream.
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => {
                return Err(WsFailure::retryable(
                    format!("elevenlabs websocket read failed: {e}"),
                    None,
                ));
            }
        };
        let value: Value = serde_json::from_str(&message).map_err(|e| {
            WsFailure::terminal(PluginError::provider(format!(
                "invalid message from the elevenlabs websocket: {e}"
            )))
        })?;
        if let Some(error) = value.get("error") {
            return Err(ws_error(error, api_key));
        }
        if let Some(audio_b64) = value.get("audio").and_then(Value::as_str) {
            // Every 4 base64 chars carry at most 3 bytes, so the encoded
            // length bounds the decoded one; reject the frame before
            // allocating a hostile-size buffer.
            let remaining = MAX_PCM_BYTES.saturating_sub(audio.len());
            if audio_b64.len().div_ceil(4).saturating_mul(3) > remaining {
                return Err(WsFailure::terminal(PluginError::provider(format!(
                    "elevenlabs websocket audio exceeds the {MAX_PCM_BYTES}-byte limit"
                ))));
            }
            let chunk = base64::engine::general_purpose::STANDARD
                .decode(audio_b64)
                .map_err(|e| {
                    WsFailure::terminal(PluginError::provider(format!(
                        "invalid base64 audio chunk from the elevenlabs websocket: {e}"
                    )))
                })?;
            audio.extend_from_slice(&chunk);
        }
        let is_final = value
            .get("isFinal")
            .or_else(|| value.get("is_final"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if is_final {
            return Ok(audio);
        }
    }
}

/// Maps a server error frame to a failure. The frame is a plain string or
/// an object carrying `status` and `message`; the message is scrubbed of
/// the API key. A 429 frame spends the retry budget like the REST path.
fn ws_error(error: &Value, api_key: &str) -> WsFailure {
    let message = match error {
        Value::String(message) if !message.trim().is_empty() => message.clone(),
        Value::Object(obj) => obj
            .get("message")
            .and_then(Value::as_str)
            .filter(|message| !message.trim().is_empty())
            .map_or_else(|| "elevenlabs stream error".to_string(), str::to_string),
        _ => "elevenlabs stream error".to_string(),
    };
    let message = mask_key(&message, api_key);
    match error.get("status").and_then(Value::as_u64) {
        Some(401 | 403) => WsFailure::terminal(PluginError::provider_typed(
            ProviderErrorKind::Auth,
            format!("authentication failed: {message}"),
        )),
        Some(429) => WsFailure::retryable(format!("rate limited: {message}"), None),
        _ => WsFailure::terminal(PluginError::provider(format!(
            "elevenlabs stream error: {message}"
        ))),
    }
}

/// Connects with the API key header; the key is never placed in the URL,
/// where proxies and access logs would record it.
async fn connect(
    url: &str,
    api_key: &str,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, WsFailure> {
    let mut request = url.into_client_request().map_err(|e| {
        WsFailure::retryable(format!("invalid elevenlabs websocket URL: {e}"), None)
    })?;
    request.headers_mut().insert(
        "xi-api-key",
        HeaderValue::from_str(api_key).map_err(|e| {
            WsFailure::terminal(PluginError::provider(format!(
                "invalid xi-api-key header value: {e}"
            )))
        })?,
    );
    let connected = timeout(CONNECT_TIMEOUT, connect_async(request)).await;
    match connected {
        Err(_) => Err(WsFailure::retryable(
            "elevenlabs websocket connect timed out",
            None,
        )),
        Ok(Err(WsError::Http(response))) => {
            let status = response.status();
            let body = response.body().as_deref().unwrap_or_default();
            if is_transient_status(status.as_u16()) {
                let message = map_http_error(status, body, api_key).to_string();
                Err(WsFailure::retryable(
                    message,
                    retry_after_secs(response.headers()),
                ))
            } else {
                Err(WsFailure::terminal(map_http_error(status, body, api_key)))
            }
        }
        Ok(Err(e)) => Err(WsFailure::retryable(
            format!("elevenlabs websocket connect failed: {e}"),
            None,
        )),
        Ok(Ok((ws, _))) => Ok(ws),
    }
}

async fn send(
    ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    text: &str,
) -> Result<(), WsFailure> {
    ws.send(Message::text(text.to_string()))
        .await
        .map_err(|e| WsFailure::retryable(format!("elevenlabs websocket send failed: {e}"), None))
}

/// Builds the `stream-input` URL from the configured base URL by swapping
/// the scheme (http → ws, https → wss). `voice_id` and `model_id` are
/// percent-encoded so free-form model IDs cannot corrupt the query string.
#[must_use]
fn ws_url(base_url: &str, voice_id: &str, config: &ElevenLabsConfig) -> String {
    let normalized = base_url.trim_end_matches('/');
    let swapped = normalized.strip_prefix("https://").map_or_else(
        || {
            normalized
                .strip_prefix("http://")
                .map_or_else(|| normalized.to_string(), |rest| format!("ws://{rest}"))
        },
        |rest| format!("wss://{rest}"),
    );
    format!(
        "{swapped}/text-to-speech/{}/stream-input?model_id={}&output_format=pcm_{}",
        utf8_percent_encode(voice_id, QUERY_ENCODE_SET),
        utf8_percent_encode(&config.model_id, QUERY_ENCODE_SET),
        config.sample_rate
    )
}

/// Splits text into chunks of at most `max_chunk` characters, preferring
/// sentence boundaries so each frame is a speech-complete unit.
#[must_use]
fn split_text(text: &str, max_chunk: usize) -> Vec<String> {
    let mut segments = Vec::new();
    let mut segment = String::new();
    let mut segment_graphemes = 0usize;
    for grapheme in text.graphemes(true) {
        segment.push_str(grapheme);
        segment_graphemes += 1;
        let ends_sentence = grapheme
            .chars()
            .last()
            .is_some_and(|ch| SENTENCE_BOUNDARIES.contains(&ch));
        if ends_sentence || segment_graphemes >= max_chunk {
            segments.push(std::mem::take(&mut segment));
            segment_graphemes = 0;
        }
    }
    if !segment.is_empty() {
        segments.push(segment);
    }
    let mut chunks: Vec<String> = Vec::new();
    for segment in segments {
        let segment_graphemes = segment.graphemes(true).count();
        if let Some(last) = chunks.last_mut()
            && last.graphemes(true).count() + segment_graphemes <= max_chunk
        {
            last.push_str(&segment);
        } else {
            chunks.push(segment);
        }
    }
    chunks
}

/// A websocket pass failure. Transport-level failures are retryable;
/// server-reported and protocol errors are terminal.
enum WsFailure {
    Retryable {
        message: String,
        /// `Retry-After` from a rate-limited handshake, when present.
        retry_after: Option<u64>,
    },
    Terminal(PluginError),
}

impl WsFailure {
    fn retryable(message: impl Into<String>, retry_after: Option<u64>) -> Self {
        Self::Retryable {
            message: message.into(),
            retry_after,
        }
    }

    fn terminal(error: PluginError) -> Self {
        Self::Terminal(error)
    }

    fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable { .. })
    }

    fn retry_after(&self) -> Option<u64> {
        match self {
            Self::Retryable { retry_after, .. } => *retry_after,
            Self::Terminal(_) => None,
        }
    }

    fn message(&self) -> String {
        match self {
            Self::Retryable { message, .. } => message.clone(),
            Self::Terminal(error) => error.to_string(),
        }
    }

    fn into_plugin_error(self) -> PluginError {
        match self {
            Self::Retryable { message, .. } => PluginError::provider(message),
            Self::Terminal(error) => error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_sentence_boundaries_and_merges_small_segments() {
        let chunks = split_text("Hello world. How are you? Fine, thanks!", 512);
        assert_eq!(chunks, vec!["Hello world. How are you? Fine, thanks!"]);
    }

    #[test]
    fn hard_splits_overlong_segments_and_keeps_chunks_bounded() {
        let text = "a".repeat(600) + "。" + &"b".repeat(600);
        let chunks = split_text(&text, 512);
        assert!(chunks.len() >= 3, "expected multiple chunks: {chunks:?}");
        assert!(
            chunks.iter().all(|chunk| chunk.chars().count() <= 512),
            "chunks exceed the bound: {chunks:?}"
        );
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn japanese_boundaries_split() {
        let chunks = split_text("こんにちは。元気ですか？はい。", 512);
        assert_eq!(chunks, vec!["こんにちは。元気ですか？はい。"]);

        let chunks = split_text("あいう。えお。かきく。", 5);
        assert_eq!(chunks, vec!["あいう。", "えお。", "かきく。"]);
    }

    #[test]
    fn empty_text_yields_no_chunks() {
        assert!(split_text("", 512).is_empty());
    }

    #[test]
    fn ws_url_swaps_scheme_and_appends_query() {
        let config = ElevenLabsConfig::default();
        let url = ws_url("https://api.elevenlabs.io/v1", "voice-1", &config);
        assert_eq!(
            url,
            "wss://api.elevenlabs.io/v1/text-to-speech/voice-1/stream-input?\
             model_id=eleven_multilingual_v2&output_format=pcm_24000"
        );

        let url = ws_url("http://127.0.0.1:9000/v1/", "voice-1", &config);
        assert_eq!(
            url,
            "ws://127.0.0.1:9000/v1/text-to-speech/voice-1/stream-input?\
             model_id=eleven_multilingual_v2&output_format=pcm_24000"
        );
    }

    #[test]
    fn ws_url_encodes_query_components() {
        let config = ElevenLabsConfig {
            model_id: "a&b=c d".to_string(),
            ..ElevenLabsConfig::default()
        };
        let url = ws_url("https://api.elevenlabs.io/v1", "my voice", &config);
        assert_eq!(
            url,
            "wss://api.elevenlabs.io/v1/text-to-speech/my%20voice/stream-input?\
             model_id=a%26b%3Dc%20d&output_format=pcm_24000"
        );
    }

    #[test]
    fn hard_split_never_splits_grapheme_clusters() {
        let text = "e\u{301}".repeat(300);
        let chunks = split_text(&text, 100);
        assert_eq!(chunks.len(), 3);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.graphemes(true).count() <= 100),
            "chunks exceed the bound: {chunks:?}"
        );
        assert_eq!(chunks.concat(), text);

        let family = "👨\u{200d}👩\u{200d}👧\u{200d}👦";
        let chunks = split_text(family, 5);
        assert_eq!(chunks, vec![family]);
    }
}
