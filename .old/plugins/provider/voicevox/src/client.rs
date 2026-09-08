use std::time::Duration;

use async_trait::async_trait;
use ene_plugin_ipc::{IpcError, TtsAudio, TtsHandler, TtsRequest};
use serde_json::{Value, json};

const DEFAULT_BASE: &str = "http://127.0.0.1:50021";

pub struct Voicevox {
    http: reqwest::Client,
}

impl Voicevox {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_mins(1))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }
}

#[async_trait]
impl TtsHandler for Voicevox {
    async fn synthesize(&self, request: TtsRequest) -> Result<TtsAudio, IpcError> {
        let base = effective_base(&request.base_url);
        let speaker = speaker_id(&request.voice);
        let query_url = format!(
            "{base}/audio_query?text={}&speaker={speaker}",
            urlencode(&request.text)
        );
        let query = self
            .http
            .post(&query_url)
            .send()
            .await
            .map_err(|err| IpcError::Call(err.to_string()))?;
        if !query.status().is_success() {
            let status = query.status();
            let body = query.text().await.unwrap_or_default();
            return Err(IpcError::Call(format!("audio_query {status}: {body}")));
        }
        let mut audio_query: Value = query
            .json()
            .await
            .map_err(|err| IpcError::Call(err.to_string()))?;
        if let Some(object) = audio_query.as_object_mut() {
            object.insert("speedScale".to_owned(), json!(1.0));
        }
        let synth_url = format!("{base}/synthesis?speaker={speaker}");
        let response = self
            .http
            .post(&synth_url)
            .json(&audio_query)
            .send()
            .await
            .map_err(|err| IpcError::Call(err.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(IpcError::Call(format!("synthesis {status}: {body}")));
        }
        let wav = response
            .bytes()
            .await
            .map_err(|err| IpcError::Call(err.to_string()))?;
        decode_wav(&wav)
    }
}

fn effective_base(request: &str) -> String {
    if let Some(base) = crate::sidecar::managed_base() {
        return base.to_owned();
    }
    if !request.is_empty() {
        return request.trim_end_matches('/').to_owned();
    }
    if let Ok(raw) = std::env::var("ENE_PROVIDER_CONFIG")
        && let Ok(value) = serde_json::from_str::<Value>(&raw)
        && let Some(url) = value.get("base_url").and_then(Value::as_str)
        && !url.is_empty()
    {
        return url.trim_end_matches('/').to_owned();
    }
    DEFAULT_BASE.to_owned()
}

fn speaker_id(voice: &str) -> u32 {
    voice.parse().unwrap_or(1)
}

fn urlencode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(*byte));
            }
            other => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(char::from(HEX[usize::from(other >> 4)]));
                out.push(char::from(HEX[usize::from(other & 0x0f)]));
            }
        }
    }
    out
}

fn decode_wav(bytes: &[u8]) -> Result<TtsAudio, IpcError> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(IpcError::Call("engine did not return WAV".to_owned()));
    }
    let sample_rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
    let bits = u16::from_le_bytes([bytes[34], bytes[35]]);
    let data_offset = find_data_chunk(bytes).unwrap_or(44);
    let pcm_bytes = bytes.get(data_offset..).unwrap_or(&[]);
    let pcm = if bits == 16 {
        pcm_bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| f32::from(i16::from_le_bytes(*chunk)) / 32768.0)
            .collect()
    } else {
        return Err(IpcError::Call(format!("unsupported WAV bits={bits}")));
    };
    Ok(TtsAudio {
        pcm,
        sample_rate: sample_rate.max(1),
        bulk: None,
    })
}

fn find_data_chunk(bytes: &[u8]) -> Option<usize> {
    let mut offset = 12_usize;
    while offset + 8 <= bytes.len() {
        let id = bytes.get(offset..offset + 4)?;
        let size = u32::from_le_bytes(bytes.get(offset + 4..offset + 8)?.try_into().ok()?);
        let start = offset.checked_add(8)?;
        if id == b"data" {
            return Some(start);
        }
        offset = start.checked_add(usize::try_from(size).ok()?)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_query_text() {
        assert_eq!(urlencode("a b"), "a%20b");
        assert_eq!(urlencode("日本語"), "%E6%97%A5%E6%9C%AC%E8%AA%9E");
    }

    #[test]
    fn speaker_defaults_to_one() {
        assert_eq!(speaker_id(""), 1);
        assert_eq!(speaker_id("3"), 3);
    }
}
