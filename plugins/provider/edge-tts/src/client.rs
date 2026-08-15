//! Edge speech WebSocket client over the host's `WebSocket` broker:
//! connection setup with browser-mimicking headers, `speech.config` +
//! `ssml` request frames, and audio collection with exponential-backoff
//! reconnects.
//!
//! The wire format mirrors the upstream python `edge-tts` client: a
//! `TrustedClientToken` + `ConnectionId` + `Sec-MS-GEC` query string, a
//! `speech.config` frame, then one `Path:ssml` frame per text chunk. Audio
//! arrives as binary frames (`Path: audio`, `Content-Type: audio/mpeg`)
//! followed by a `Path:turn.end` text frame.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ene_plugin_broker::{BrokerClientError, WebSocketEvent, WebSocketSession};
use sha2::{Digest, Sha256};
use tokio::time::timeout;
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
/// Origin of the Edge Read Aloud extension. Upstream edge-tts bumps this ID
/// when the service starts rejecting the older fingerprint; keep in sync.
const ORIGIN_VALUE: &str = "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold";

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

/// Clock skew between the service's `Date` header and this machine, applied
/// when generating `Sec-MS-GEC` tokens. Corrected on HTTP 403, which the
/// service sends when the token misses its 5-minute validity window.
static CLOCK_SKEW_SECS: AtomicI64 = AtomicI64::new(0);

/// Synthesizes every chunk over one connection, reconnecting from the
/// failed chunk with exponential backoff when a retryable transport error
/// occurs. The retry budget in `config.max_retries` is shared across all
/// chunks of the request. Returns the concatenated MP3 stream.
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
    ws.send_text(&speech_config())
        .await
        .map_err(|e| EdgeError::Send(e.to_string()))?;
    let mut index = start_index;
    while index < chunks.len() {
        let chunk_start = mp3.len();
        ws.send_text(&synthesis_request(&build_ssml(config, &chunks[index])))
            .await
            .map_err(|e| EdgeError::Send(e.to_string()))?;
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

/// Connects to the configured endpoint through the host's WebSocket broker
/// with the browser-mimicking headers.
async fn connect(config: &EdgeTtsConfig) -> Result<WebSocketSession, EdgeError> {
    let request_id = uuid::Uuid::new_v4().simple().to_string();
    let url = format!(
        "{}?TrustedClientToken={}&ConnectionId={}&Sec-MS-GEC={}&Sec-MS-GEC-Version={}",
        ensure_path(&config.endpoint_url),
        TRUSTED_CLIENT_TOKEN,
        request_id,
        sec_ms_gec_token(),
        SEC_MS_GEC_VERSION,
    );
    let headers = vec![
        ("User-Agent".to_string(), USER_AGENT_VALUE.to_string()),
        ("Origin".to_string(), ORIGIN_VALUE.to_string()),
        (
            "Accept-Encoding".to_string(),
            "gzip, deflate, br, zstd".to_string(),
        ),
        ("Accept-Language".to_string(), "en-US,en;q=0.9".to_string()),
        ("Pragma".to_string(), "no-cache".to_string()),
        ("Cache-Control".to_string(), "no-cache".to_string()),
    ];
    // The `muid` cookie is stripped by the host with all cookie headers;
    // the service accepts connections without it.

    timeout(CONNECT_TIMEOUT, crate::broker::broker().open(&url, headers))
        .await
        .map_err(|_| EdgeError::Timeout)?
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
         {{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":{{\"sentenceBoundaryEnabled\":\"true\",\"wordBoundaryEnabled\":\"false\"}},\"outputFormat\":\"{OUTPUT_FORMAT}\"}}}}}}}}\r\n",
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
async fn collect_audio(ws: &mut WebSocketSession, audio: &mut Vec<u8>) -> Result<(), EdgeError> {
    let mut audio_received = false;
    loop {
        let event = timeout(MESSAGE_TIMEOUT, ws.recv())
            .await
            .map_err(|_| EdgeError::Timeout)?
            .map_err(|e| classify_connect_error(&e))?;
        match event {
            WebSocketEvent::Text(text) => match text_path(&text) {
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
            WebSocketEvent::Binary(bytes) => {
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
            WebSocketEvent::Closed { .. } | WebSocketEvent::Error { .. } => {
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

/// Maps a handshake failure to a typed error. 408/429 (rate limiting) are
/// transport-level failures and retryable; 403 means the `Sec-MS-GEC` token
/// was rejected, so the clock is re-synced from the response `Date` header
/// and the error is retryable; any other 4xx is a permanent rejection.
fn classify_connect_error(error: &BrokerClientError) -> EdgeError {
    match error {
        BrokerClientError::HttpStatus { status, message } => match status {
            Some(403) => {
                apply_server_clock_skew_message(message);
                EdgeError::Rejected(403)
            }
            Some(408 | 429) | None => EdgeError::Connect(message.clone()),
            Some(status) => EdgeError::Rejected(*status),
        },
        other => EdgeError::Connect(other.to_string()),
    }
}

/// Corrects the process clock from the handshake error's `Date` header (the
/// host forwards it in the message) so the next `Sec-MS-GEC` token lands in
/// the service's 5-minute window.
fn apply_server_clock_skew_message(message: &str) {
    let Some(date) = message.split_once(" Date: ").map(|(_, date)| date) else {
        return;
    };
    let Some(skew) = clock_skew_secs(date, unix_now_secs()) else {
        return;
    };
    CLOCK_SKEW_SECS.store(skew, Ordering::Relaxed);
}

/// Difference in seconds between the server's `Date` header and the local
/// clock; positive when the server is ahead.
fn clock_skew_secs(server_date: &str, client_now_secs: u64) -> Option<i64> {
    let server = chrono::DateTime::parse_from_rfc2822(server_date)
        .ok()?
        .timestamp();
    Some(server - i64::try_from(client_now_secs).ok()?)
}

/// `Sec-MS-GEC` token: uppercase SHA-256 hex of
/// `<FILETIME ticks floored to 5 minutes><TrustedClientToken>`, computed
/// from the clock-skew-corrected time.
fn sec_ms_gec_token() -> String {
    sec_ms_gec_token_at(unix_now_secs(), CLOCK_SKEW_SECS.load(Ordering::Relaxed))
}

fn sec_ms_gec_token_at(now_secs: u64, skew_secs: i64) -> String {
    let now = now_secs.saturating_add_signed(skew_secs);
    let ticks = (now.saturating_add(WIN_EPOCH_SECS) / 300 * 300) * 10_000_000;
    let mut hasher = Sha256::new();
    hasher.update(ticks.to_string());
    hasher.update(TRUSTED_CLIENT_TOKEN);
    hex::encode_upper(hasher.finalize())
}

/// JavaScript-style UTC date string the service expects in
/// `X-Timestamp` headers.
fn date_to_string() -> String {
    date_to_string_at(chrono::Utc::now())
}

fn date_to_string_at(now: chrono::DateTime<chrono::Utc>) -> String {
    now.format("%a %b %d %Y %H:%M:%S GMT+0000 (Coordinated Universal Time)")
        .to_string()
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

async fn backoff(attempt: u32) {
    let delay = BACKOFF_BASE_MS
        .saturating_mul(1u64 << attempt.min(5))
        .min(BACKOFF_MAX_MS);
    tokio::time::sleep(Duration::from_millis(delay)).await;
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use super::*;
    /// 2026-08-05 01:23:45 UTC; golden token from the upstream python
    /// edge-tts `DRM.generate_sec_ms_gec` algorithm.
    const PINNED_NOW_SECS: u64 = 1_785_893_025;

    #[test]
    fn sec_ms_gec_token_matches_reference_implementation() {
        assert_eq!(
            sec_ms_gec_token_at(PINNED_NOW_SECS, 0),
            "F9597B1DFDCF15DAD67B92BEAE1712C857F2627C368B6BCFDD09CF2307E5E140"
        );
    }

    #[test]
    fn sec_ms_gec_token_shifts_with_clock_skew() {
        // +300 s moves the token into the next 5-minute window.
        assert_ne!(
            sec_ms_gec_token_at(PINNED_NOW_SECS, 0),
            sec_ms_gec_token_at(PINNED_NOW_SECS, 300)
        );
    }

    #[test]
    fn date_to_string_matches_javascript_style() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-05T01:23:45Z")
            .expect("parse")
            .with_timezone(&chrono::Utc);
        assert_eq!(
            date_to_string_at(now),
            "Wed Aug 05 2026 01:23:45 GMT+0000 (Coordinated Universal Time)"
        );
    }

    #[test]
    fn clock_skew_is_positive_when_server_ahead() {
        assert_eq!(
            clock_skew_secs("Wed, 05 Aug 2026 01:28:45 GMT", PINNED_NOW_SECS),
            Some(300)
        );
        assert_eq!(
            clock_skew_secs("Wed, 05 Aug 2026 01:23:45 GMT", PINNED_NOW_SECS),
            Some(0)
        );
        assert_eq!(clock_skew_secs("not a date", PINNED_NOW_SECS), None);
    }

    fn http_error(status: u16) -> BrokerClientError {
        BrokerClientError::HttpStatus {
            status: Some(status),
            message: format!("WebSocket handshake failed: HTTP {status}"),
        }
    }

    #[test]
    fn classifies_rate_limits_as_retryable_connect_errors() {
        assert!(matches!(
            classify_connect_error(&http_error(429)),
            EdgeError::Connect(_)
        ));
        assert!(matches!(
            classify_connect_error(&http_error(408)),
            EdgeError::Connect(_)
        ));
    }

    #[test]
    fn classifies_other_4xx_as_permanent_rejections() {
        assert!(matches!(
            classify_connect_error(&http_error(404)),
            EdgeError::Rejected(404)
        ));
        assert!(matches!(
            classify_connect_error(&http_error(400)),
            EdgeError::Rejected(400)
        ));
    }

    #[test]
    fn classifies_403_as_retryable() {
        let err = classify_connect_error(&http_error(403));
        assert!(matches!(err, EdgeError::Rejected(403)));
        assert!(err.retryable());
    }
}
