//! Edge speech WebSocket client: connection setup with browser-mimicking
//! headers, `speech.config` + `ssml` request frames, and audio collection
//! with exponential-backoff reconnects.
//!
//! The wire format mirrors the upstream python `edge-tts` client: a
//! `TrustedClientToken` + `ConnectionId` + `Sec-MS-GEC` query string, a
//! `speech.config` frame, then one `Path:ssml` frame per text chunk. Audio
//! arrives as binary frames (`Path: audio`, `Content-Type: audio/mpeg`)
//! followed by a `Path:turn.end` text frame.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::{SinkExt, StreamExt};
use http::header::{HeaderName, HeaderValue, ORIGIN, USER_AGENT};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tracing::debug;

use crate::config::EdgeTtsConfig;
use crate::error::EdgeError;
use crate::protocol::{parse_binary_frame, text_path};
use crate::ssml::build_ssml;

/// Trusted client token issued to the Edge Read Aloud extension; the
/// service requires it on every connection.
const TRUSTED_CLIENT_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
/// Chromium version the UA and `Sec-MS-GEC-Version` advertise; the service
/// rejects requests whose version predates its supported set.
const SEC_MS_GEC_VERSION: &str = "1-143.0.3650.75";
const USER_AGENT_VALUE: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36 Edg/143.0.0.0";
const ORIGIN_VALUE: &str = "chrome-extension://jdiccldimpdaibocbdbgnbgflacejaoo";

/// Seconds between the Unix epoch and the Windows FILETIME epoch; the
/// `Sec-MS-GEC` token is hashed in FILETIME units.
const WIN_EPOCH_SECS: u64 = 11_644_473_600;
/// The 48 kbps mono MP3 format the service is asked for; the fixed bitrate
/// keeps every chunk at 24 kHz mono.
const OUTPUT_FORMAT: &str = "audio-24khz-48kbitrate-mono-mp3";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MESSAGE_TIMEOUT: Duration = Duration::from_mins(1);
const BACKOFF_BASE_MS: u64 = 250;
const BACKOFF_MAX_MS: u64 = 5_000;

/// Synthesizes every chunk over one connection, reconnecting from the
/// failed chunk with exponential backoff when a retryable transport error
/// occurs. Returns the concatenated MP3 stream.
///
/// # Errors
///
/// Returns [`EdgeError`] from the last attempt when the retry budget is
/// exhausted, or immediately for non-retryable failures.
pub async fn synthesize(config: &EdgeTtsConfig, chunks: &[String]) -> Result<Vec<u8>, EdgeError> {
    let mut mp3 = Vec::new();
    let mut chunk_index = 0usize;
    let mut attempts = 0u32;
    while chunk_index < chunks.len() {
        match synthesize_pass(config, chunks, chunk_index, &mut mp3).await {
            Ok(done) => chunk_index += done,
            Err(e) if e.retryable() && attempts < config.max_retries => {
                attempts += 1;
                backoff(attempts).await;
                debug!(component = "EdgeTtsClient", attempt = attempts, error = %e, "reconnecting");
            }
            Err(e) => return Err(e),
        }
    }
    Ok(mp3)
}

/// Runs one connection pass starting at `start_index`, appending MP3 bytes
/// to `mp3`. Returns how many chunks completed. A retryable error leaves
/// `mp3` truncated to the last chunk boundary so the caller can resume.
async fn synthesize_pass(
    config: &EdgeTtsConfig,
    chunks: &[String],
    start_index: usize,
    mp3: &mut Vec<u8>,
) -> Result<usize, EdgeError> {
    let mut ws = connect(config).await?;
    send_frame(&mut ws, &speech_config()).await?;
    let mut index = start_index;
    while index < chunks.len() {
        let chunk_start = mp3.len();
        send_frame(
            &mut ws,
            &synthesis_request(&build_ssml(config, &chunks[index])),
        )
        .await?;
        match collect_audio(&mut ws, mp3).await {
            Ok(()) => index += 1,
            Err(e) if e.retryable() => {
                mp3.truncate(chunk_start);
                return Err(e);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(index - start_index)
}

/// Connects to the configured endpoint with the browser-mimicking headers.
async fn connect(
    config: &EdgeTtsConfig,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, EdgeError> {
    let request_id = uuid::Uuid::new_v4().simple().to_string();
    let url = format!(
        "{}?TrustedClientToken={}&ConnectionId={}&Sec-MS-GEC={}&Sec-MS-GEC-Version={}",
        ensure_path(&config.endpoint_url),
        TRUSTED_CLIENT_TOKEN,
        request_id,
        sec_ms_gec_token(),
        SEC_MS_GEC_VERSION,
    );
    let mut request = url
        .into_client_request()
        .map_err(|e| EdgeError::Connect(format!("invalid endpoint URL: {e}")))?;
    let headers = request.headers_mut();
    headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
    headers.insert(ORIGIN, HeaderValue::from_static(ORIGIN_VALUE));
    headers.insert(
        HeaderName::from_static("accept-encoding"),
        HeaderValue::from_static("gzip, deflate, br, zstd"),
    );
    headers.insert(
        HeaderName::from_static("accept-language"),
        HeaderValue::from_static("en-US,en;q=0.9"),
    );
    headers.insert(
        HeaderName::from_static("pragma"),
        HeaderValue::from_static("no-cache"),
    );
    headers.insert(
        HeaderName::from_static("cache-control"),
        HeaderValue::from_static("no-cache"),
    );
    headers.insert(
        HeaderName::from_static("cookie"),
        HeaderValue::from_str(&format!("muid={};", uuid::Uuid::new_v4().simple()))
            .map_err(|e| EdgeError::Connect(format!("invalid muid header: {e}")))?,
    );

    timeout(CONNECT_TIMEOUT, connect_async(request))
        .await
        .map_err(|_| EdgeError::Timeout)?
        .map(|(ws, _)| ws)
        .map_err(|e| classify_connect_error(&e))
}

/// Ensures the endpoint has a path: a host-only `ws://host` would produce
/// the invalid request target `GET ?query HTTP/1.1`.
fn ensure_path(endpoint: &str) -> String {
    let mut url = endpoint.to_string();
    if endpoint
        .split_once("://")
        .is_some_and(|(_, rest)| !rest.contains('/'))
    {
        url.push('/');
    }
    url
}

/// `speech.config` frame: JSON synthesis context selecting the MP3 output
/// format. No `X-RequestId`; the service keys responses to the request id
/// of the `ssml` frame.
fn speech_config() -> String {
    format!(
        "X-Timestamp:{}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n\
         {{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":{{\"sentenceBoundaryEnabled\":\"true\",\"wordBoundaryEnabled\":\"false\"}},\"outputFormat\":\"{OUTPUT_FORMAT}\"}}}}}}}}",
        date_to_string()
    )
}

/// `Path:ssml` frame carrying one SSML document. The trailing `Z` on
/// `X-Timestamp` is required by the service (it echoes the header back).
/// The SSML body is appended, not formatted, because it contains
/// user-derived text that `format!` would treat as placeholders.
fn synthesis_request(ssml: &str) -> String {
    let mut frame = format!(
        "X-RequestId:{}\r\nContent-Type:application/ssml+xml\r\nX-Timestamp:{}Z\r\nPath:ssml\r\n\r\n",
        uuid::Uuid::new_v4().simple(),
        date_to_string()
    );
    frame.push_str(ssml);
    frame
}

/// Reads frames until `turn.end`, appending MP3 payloads to `audio`.
async fn collect_audio<W>(ws: &mut W, audio: &mut Vec<u8>) -> Result<(), EdgeError>
where
    W: futures::Stream<Item = Result<Message, WsError>> + Unpin,
{
    let mut audio_received = false;
    loop {
        let message = timeout(MESSAGE_TIMEOUT, ws.next())
            .await
            .map_err(|_| EdgeError::Timeout)?
            .ok_or_else(|| EdgeError::Connect("connection closed before turn.end".to_string()))?
            .map_err(|e| classify_connect_error(&e))?;
        match message {
            Message::Text(text) => match text_path(&text) {
                Some("turn.start" | "response" | "audio.metadata") => {}
                Some("turn.end") => break,
                Some(other) => {
                    return Err(EdgeError::Protocol(format!(
                        "unexpected text path: {other}"
                    )));
                }
                None => {
                    return Err(EdgeError::Protocol(
                        "text frame without Path header".to_string(),
                    ));
                }
            },
            Message::Binary(bytes) => {
                let frame = parse_binary_frame(&bytes)?;
                if frame.path != "audio" {
                    return Err(EdgeError::Protocol(format!(
                        "unexpected binary path: {}",
                        frame.path
                    )));
                }
                match frame.content_type {
                    Some("audio/mpeg") => {
                        if frame.payload.is_empty() {
                            return Err(EdgeError::Protocol(
                                "audio frame without payload".to_string(),
                            ));
                        }
                        audio_received = true;
                        if audio.len().saturating_add(frame.payload.len())
                            > crate::audio::MAX_MP3_BYTES
                        {
                            return Err(EdgeError::TooLarge {
                                max: crate::audio::MAX_MP3_BYTES,
                            });
                        }
                        audio.extend_from_slice(frame.payload);
                    }
                    Some(other) => {
                        return Err(EdgeError::Protocol(format!(
                            "unexpected audio content type: {other}"
                        )));
                    }
                    None => {
                        // The service closes the audio stream with a
                        // Content-Type-less empty binary frame.
                        if !frame.payload.is_empty() {
                            return Err(EdgeError::Protocol(
                                "audio frame without Content-Type but with data".to_string(),
                            ));
                        }
                    }
                }
            }
            Message::Ping(_) | Message::Pong(_) => {}
            Message::Close(_) | Message::Frame(_) => {
                return Err(EdgeError::Connect(
                    "connection closed before turn.end".to_string(),
                ));
            }
        }
    }
    if !audio_received {
        return Err(EdgeError::NoAudio);
    }
    Ok(())
}

async fn send_frame<W>(ws: &mut W, frame: &str) -> Result<(), EdgeError>
where
    W: futures::Sink<Message, Error = WsError> + Unpin,
{
    ws.send(Message::text(frame.to_string()))
        .await
        .map_err(|e| EdgeError::Send(e.to_string()))
}

/// Maps a handshake failure to a typed error: HTTP 4xx means the request
/// itself was rejected (not retryable), everything else is a transport
/// failure (retryable).
fn classify_connect_error(error: &WsError) -> EdgeError {
    match error {
        WsError::Http(response) if response.status().is_client_error() => {
            EdgeError::Rejected(response.status().as_u16())
        }
        other => EdgeError::Connect(other.to_string()),
    }
}

/// `Sec-MS-GEC` token: uppercase SHA-256 hex of
/// `<FILETIME ticks floored to 5 minutes><TrustedClientToken>`.
fn sec_ms_gec_token() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let ticks = (now.saturating_add(WIN_EPOCH_SECS) / 300 * 300) * 10_000_000;
    let mut hasher = Sha256::new();
    hasher.update(ticks.to_string());
    hasher.update(TRUSTED_CLIENT_TOKEN);
    hex::encode_upper(hasher.finalize())
}

/// JavaScript-style UTC date string the service expects in
/// `X-Timestamp` headers.
fn date_to_string() -> String {
    chrono::Utc::now()
        .format("%a %b %d %Y %H:%M:%S GMT+0000 (Coordinated Universal Time)")
        .to_string()
}

async fn backoff(attempt: u32) {
    let delay = BACKOFF_BASE_MS
        .saturating_mul(1u64 << attempt.min(5))
        .min(BACKOFF_MAX_MS);
    tokio::time::sleep(Duration::from_millis(delay)).await;
}
