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
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use ene_plugin::{PluginError, ProviderErrorKind};

use crate::client::{
    CONNECT_TIMEOUT, MAX_ATTEMPTS, MAX_DELAY, MAX_PCM_BYTES, REQUEST_TIMEOUT, backoff_delay,
    map_http_error,
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

/// Synthesizes `text` over the `stream-input` WebSocket and returns WAV
/// bytes. The whole request is retried from scratch on retryable failures.
///
/// # Errors
///
/// Returns a provider error for server-reported errors (401/403 → `Auth`,
/// 429 → `RateLimit`), malformed audio frames, and transport failures that
/// exhaust the retry budget. A typed `Truncated` error surfaces when the
/// collected PCM ends mid-sample.
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
                let delay = backoff_delay(attempt).min(MAX_DELAY);
                tracing::warn!(
                    component = "ene-plugin-elevenlabs",
                    attempt = next,
                    delay_ms = delay.as_millis() as u64,
                    error = failure.message(),
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

    let audio = match timeout(REQUEST_TIMEOUT, collect_audio(&mut ws)).await {
        Err(_) => {
            return Err(WsFailure::retryable(
                "elevenlabs websocket timed out before the final frame",
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
) -> Result<Vec<u8>, WsFailure> {
    let mut audio = Vec::new();
    loop {
        let message = match timeout(MESSAGE_TIMEOUT, ws.next()).await {
            Err(_) => {
                return Err(WsFailure::retryable(
                    "elevenlabs websocket timed out waiting for an audio message",
                ));
            }
            Ok(Some(Ok(Message::Text(text)))) => text,
            Ok(Some(Ok(Message::Close(_))) | None) => {
                return Err(WsFailure::retryable(
                    "elevenlabs websocket closed before the final frame",
                ));
            }
            // Binary/ping/pong frames carry nothing usable; pings are
            // answered automatically by the stream.
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => {
                return Err(WsFailure::retryable(format!(
                    "elevenlabs websocket read failed: {e}"
                )));
            }
        };
        let value: Value = serde_json::from_str(&message).map_err(|e| {
            WsFailure::terminal(PluginError::provider(format!(
                "invalid message from the elevenlabs websocket: {e}"
            )))
        })?;
        if let Some(error) = value.get("error") {
            return Err(WsFailure::terminal(ws_error(error)));
        }
        if let Some(audio_b64) = value.get("audio").and_then(Value::as_str) {
            let chunk = base64::engine::general_purpose::STANDARD
                .decode(audio_b64)
                .map_err(|e| {
                    WsFailure::terminal(PluginError::provider(format!(
                        "invalid base64 audio chunk from the elevenlabs websocket: {e}"
                    )))
                })?;
            audio.extend_from_slice(&chunk);
            if audio.len() > MAX_PCM_BYTES {
                return Err(WsFailure::terminal(PluginError::provider(format!(
                    "elevenlabs websocket audio exceeds the {MAX_PCM_BYTES}-byte limit"
                ))));
            }
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

/// Maps a server error frame to a typed error. The frame is a plain string
/// or an object carrying `status` and `message`.
fn ws_error(error: &Value) -> PluginError {
    let message = match error {
        Value::String(message) if !message.trim().is_empty() => message.clone(),
        Value::Object(obj) => obj
            .get("message")
            .and_then(Value::as_str)
            .filter(|message| !message.trim().is_empty())
            .map_or_else(|| "elevenlabs stream error".to_string(), str::to_string),
        _ => "elevenlabs stream error".to_string(),
    };
    match error.get("status").and_then(Value::as_u64) {
        Some(401 | 403) => PluginError::provider_typed(
            ProviderErrorKind::Auth,
            format!("authentication failed: {message}"),
        ),
        Some(429) => PluginError::provider_typed(
            ProviderErrorKind::RateLimit,
            format!("rate limited: {message}"),
        ),
        _ => PluginError::provider(format!("elevenlabs stream error: {message}")),
    }
}

/// Connects with the API key header; the key is never placed in the URL,
/// where proxies and access logs would record it.
async fn connect(
    url: &str,
    api_key: &str,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, WsFailure> {
    let mut request = url
        .into_client_request()
        .map_err(|e| WsFailure::retryable(format!("invalid elevenlabs websocket URL: {e}")))?;
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
        )),
        Ok(Err(WsError::Http(response))) => {
            let body = response.body().as_deref().unwrap_or_default();
            Err(WsFailure::terminal(map_http_error(response.status(), body)))
        }
        Ok(Err(e)) => Err(WsFailure::retryable(format!(
            "elevenlabs websocket connect failed: {e}"
        ))),
        Ok(Ok((ws, _))) => Ok(ws),
    }
}

async fn send(
    ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    text: &str,
) -> Result<(), WsFailure> {
    ws.send(Message::text(text.to_string()))
        .await
        .map_err(|e| WsFailure::retryable(format!("elevenlabs websocket send failed: {e}")))
}

/// Builds the `stream-input` URL from the configured base URL by swapping
/// the scheme (http → ws, https → wss).
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
        "{swapped}/text-to-speech/{voice_id}/stream-input?model_id={}&output_format=pcm_{}",
        config.model_id, config.sample_rate
    )
}

/// Splits text into chunks of at most `max_chunk` characters, preferring
/// sentence boundaries so each frame is a speech-complete unit.
#[must_use]
fn split_text(text: &str, max_chunk: usize) -> Vec<String> {
    let mut segments = Vec::new();
    let mut segment = String::new();
    for ch in text.chars() {
        segment.push(ch);
        if SENTENCE_BOUNDARIES.contains(&ch) || segment.chars().count() >= max_chunk {
            segments.push(std::mem::take(&mut segment));
        }
    }
    if !segment.is_empty() {
        segments.push(segment);
    }
    let mut chunks: Vec<String> = Vec::new();
    for segment in segments {
        if let Some(last) = chunks.last_mut()
            && last.chars().count() + segment.chars().count() <= max_chunk
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
    Retryable(String),
    Terminal(PluginError),
}

impl WsFailure {
    fn retryable(message: impl Into<String>) -> Self {
        Self::Retryable(message.into())
    }

    fn terminal(error: PluginError) -> Self {
        Self::Terminal(error)
    }

    fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable(_))
    }

    fn message(&self) -> String {
        match self {
            Self::Retryable(message) => message.clone(),
            Self::Terminal(error) => error.to_string(),
        }
    }

    fn into_plugin_error(self) -> PluginError {
        match self {
            Self::Retryable(message) => PluginError::provider(message),
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
}
