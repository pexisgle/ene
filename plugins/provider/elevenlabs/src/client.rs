use std::time::Duration;

use async_trait::async_trait;
use ene_plugin_ipc::{IpcError, TtsAudio, TtsHandler, TtsRequest};
use serde_json::{Value, json};

const DEFAULT_BASE: &str = "https://api.elevenlabs.io/v1";
const DEFAULT_VOICE: &str = "21m00Tcm4TlvDq8ikWAM";
const SAMPLE_RATE: u32 = 24_000;

pub struct ElevenLabs {
    http: reqwest::Client,
}

impl ElevenLabs {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_mins(2))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }
}

#[async_trait]
impl TtsHandler for ElevenLabs {
    async fn synthesize(&self, request: TtsRequest) -> Result<TtsAudio, IpcError> {
        let voice = if request.voice.is_empty() {
            DEFAULT_VOICE
        } else {
            request.voice.as_str()
        };
        let url = format!(
            "{}/text-to-speech/{voice}/stream?output_format=pcm_{SAMPLE_RATE}",
            effective_base(&request.base_url)
        );
        let body = json!({
            "text": request.text,
            "model_id": effective_model(&request.model, "eleven_multilingual_v2"),
        });
        let mut http = self.http.post(&url).json(&body);
        if !request.auth.api_key.is_empty() {
            http = http.header("xi-api-key", &request.auth.api_key);
        }
        let response = http
            .send()
            .await
            .map_err(|err| IpcError::Call(err.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(IpcError::Call(format!("{status}: {body}")));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|err| IpcError::Call(err.to_string()))?;
        Ok(TtsAudio {
            pcm: pcm16le_to_f32(&bytes),
            sample_rate: SAMPLE_RATE,
        })
    }
}

fn effective_base(request: &str) -> String {
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

fn effective_model(request: &str, fallback: &str) -> String {
    if request.is_empty() || request == "echo" {
        fallback.to_owned()
    } else {
        request.to_owned()
    }
}

fn pcm16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| {
            let sample = i16::from_le_bytes(*chunk);
            f32::from(sample) / 32768.0
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_converts_silence() {
        let pcm = pcm16le_to_f32(&[0, 0, 0, 0]);
        assert_eq!(pcm, vec![0.0, 0.0]);
    }
}
