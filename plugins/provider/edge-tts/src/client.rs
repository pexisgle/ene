use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ene_plugin_ipc::{IpcError, TtsAudio, TtsHandler, TtsRequest};
use futures::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use uuid::Uuid;

use crate::protocol::{parse_binary_frame, text_path};
use crate::ssml::{build_ssml, chunk_text, escape_xml, sanitize};

const TRUSTED_CLIENT_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
const SEC_MS_GEC_VERSION: &str = "1-143.0.3650.75";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36 Edg/143.0.0.0";
const ORIGIN: &str = "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold";
const DEFAULT_ENDPOINT: &str =
    "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1";
const OUTPUT_FORMAT: &str = "raw-24khz-16bit-mono-pcm";
const SAMPLE_RATE: u32 = 24_000;
const WIN_EPOCH_SECS: u64 = 11_644_473_600;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MESSAGE_TIMEOUT: Duration = Duration::from_mins(1);

static CLOCK_SKEW_SECS: AtomicI64 = AtomicI64::new(0);

pub struct EdgeTts;

#[async_trait]
impl TtsHandler for EdgeTts {
    async fn synthesize(&self, request: TtsRequest) -> Result<TtsAudio, IpcError> {
        let voice = if request.voice.is_empty() {
            "ja-JP-NanamiNeural"
        } else {
            request.voice.as_str()
        };
        let locale = locale_from_voice(voice);
        let prepared = escape_xml(&sanitize(&request.text));
        let chunks = chunk_text(&prepared);
        if chunks.is_empty() {
            return Ok(TtsAudio {
                pcm: Vec::new(),
                sample_rate: SAMPLE_RATE,
                bulk: None,
            });
        }
        let endpoint = if request.base_url.is_empty() {
            DEFAULT_ENDPOINT.to_owned()
        } else {
            request.base_url.clone()
        };
        let mut pcm_bytes = Vec::new();
        for chunk in chunks {
            let ssml = build_ssml(voice, &locale, &chunk);
            collect_chunk(&endpoint, &ssml, &mut pcm_bytes).await?;
        }
        Ok(TtsAudio {
            pcm: pcm16le_to_f32(&pcm_bytes),
            sample_rate: SAMPLE_RATE,
            bulk: None,
        })
    }
}

async fn collect_chunk(endpoint: &str, ssml: &str, pcm: &mut Vec<u8>) -> Result<(), IpcError> {
    let (mut ws, _) = timeout(CONNECT_TIMEOUT, connect(endpoint))
        .await
        .map_err(|_| IpcError::Call("edge-tts connect timeout".to_owned()))?
        .map_err(|err| IpcError::Call(err.to_string()))?;
    ws.send(Message::Text(speech_config().into()))
        .await
        .map_err(|err| IpcError::Call(err.to_string()))?;
    ws.send(Message::Text(synthesis_request(ssml).into()))
        .await
        .map_err(|err| IpcError::Call(err.to_string()))?;
    let mut audio = false;
    loop {
        let event = timeout(MESSAGE_TIMEOUT, ws.next())
            .await
            .map_err(|_| IpcError::Call("edge-tts message timeout".to_owned()))?
            .ok_or_else(|| IpcError::Call("edge-tts closed".to_owned()))?
            .map_err(|err| IpcError::Call(err.to_string()))?;
        match event {
            Message::Text(text) => match text_path(&text) {
                Some("turn.end") => break,
                Some(other) if !matches!(other, "turn.start" | "response" | "audio.metadata") => {
                    return Err(IpcError::Call(format!("unexpected text path: {other}")));
                }
                _ => {}
            },
            Message::Binary(bytes) => {
                let frame = parse_binary_frame(&bytes).map_err(IpcError::Call)?;
                if frame.path != "audio" {
                    continue;
                }
                if !frame.payload.is_empty() {
                    audio = true;
                    pcm.extend_from_slice(frame.payload);
                }
            }
            Message::Close(_) => {
                return Err(IpcError::Call("edge-tts closed before turn.end".to_owned()));
            }
            _ => {}
        }
    }
    if !audio {
        return Err(IpcError::Call("edge-tts returned no audio".to_owned()));
    }
    Ok(())
}

async fn connect(
    endpoint: &str,
) -> Result<
    (
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
    ),
    IpcError,
> {
    let request_id = Uuid::new_v4().simple().to_string();
    let url = format!(
        "{}?TrustedClientToken={TRUSTED_CLIENT_TOKEN}&ConnectionId={request_id}&Sec-MS-GEC={}&Sec-MS-GEC-Version={SEC_MS_GEC_VERSION}",
        ensure_path(endpoint),
        sec_ms_gec_token(),
    );
    let mut request = url
        .into_client_request()
        .map_err(|err| IpcError::Call(err.to_string()))?;
    let headers = request.headers_mut();
    headers.insert(
        "User-Agent",
        USER_AGENT
            .parse()
            .map_err(|err| IpcError::Call(format!("{err}")))?,
    );
    headers.insert(
        "Origin",
        ORIGIN
            .parse()
            .map_err(|err| IpcError::Call(format!("{err}")))?,
    );
    tokio_tungstenite::connect_async(request)
        .await
        .map_err(|err| IpcError::Call(err.to_string()))
}

fn ensure_path(endpoint: &str) -> String {
    let mut url = endpoint.to_owned();
    if endpoint
        .split_once("://")
        .is_some_and(|(_, rest)| !rest.contains('/'))
    {
        url.push('/');
    }
    url
}

fn speech_config() -> String {
    format!(
        "X-Timestamp:{}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n\
         {{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":{{\"sentenceBoundaryEnabled\":\"true\",\"wordBoundaryEnabled\":\"false\"}},\"outputFormat\":\"{OUTPUT_FORMAT}\"}}}}}}}}\r\n",
        date_to_string()
    )
}

fn synthesis_request(ssml: &str) -> String {
    let mut frame = format!(
        "X-RequestId:{}\r\nContent-Type:application/ssml+xml\r\nX-Timestamp:{}Z\r\nPath:ssml\r\n\r\n",
        Uuid::new_v4().simple(),
        date_to_string()
    );
    frame.push_str(ssml);
    frame
}

fn sec_ms_gec_token() -> String {
    let now = unix_now_secs().saturating_add_signed(CLOCK_SKEW_SECS.load(Ordering::Relaxed));
    let ticks = now.saturating_add(WIN_EPOCH_SECS) / 300 * 300 * 10_000_000;
    let mut hasher = Sha256::new();
    hasher.update(ticks.to_string());
    hasher.update(TRUSTED_CLIENT_TOKEN);
    hex::encode_upper(hasher.finalize())
}

fn date_to_string() -> String {
    chrono::Utc::now()
        .format("%a %b %d %Y %H:%M:%S GMT+0000 (Coordinated Universal Time)")
        .to_string()
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn locale_from_voice(voice: &str) -> String {
    let parts: Vec<&str> = voice.split('-').collect();
    if parts.len() >= 2 {
        format!("{}-{}", parts[0], parts[1])
    } else {
        "ja-JP".to_owned()
    }
}

fn pcm16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| f32::from(i16::from_le_bytes(*chunk)) / 32768.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_from_short_voice() {
        assert_eq!(locale_from_voice("ja-JP-NanamiNeural"), "ja-JP");
        assert_eq!(locale_from_voice("en-US-AvaNeural"), "en-US");
    }

    #[test]
    fn sec_ms_gec_is_uppercase_hex() {
        let token = sec_ms_gec_token();
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(
            token
                .bytes()
                .any(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        );
    }
}
