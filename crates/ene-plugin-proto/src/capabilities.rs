//! Plugin capability declarations.
//!
//! A plugin advertises its capabilities during the handshake so the host
//! can route tool registrations, LLM provider factories, and future
//! TTS/STT providers appropriately.

use serde::{Deserialize, Serialize};

/// Capabilities advertised by a plugin during the handshake.
///
/// The host inspects this struct after a successful `HandshakeAck` to
/// decide which registries to populate:
///
/// - `tools` → merged into the composite tool registry
/// - `llm_providers` → registered as `LlmProviderFactory` entries
/// - `tts_providers` / `stt_providers` → reserved for future use
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCapabilities {
    /// Number of tools this plugin provides (call `ListTools` for specs).
    #[serde(default)]
    pub tools: usize,

    /// LLM providers exposed by this plugin.
    #[serde(default)]
    pub llm_providers: Vec<LlmProviderSpec>,

    /// TTS providers (reserved for future use).
    #[serde(default)]
    pub tts_providers: Vec<TtsProviderSpec>,

    /// STT providers (reserved for future use).
    #[serde(default)]
    pub stt_providers: Vec<SttProviderSpec>,
}

/// Specification of an LLM provider exposed by a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmProviderSpec {
    /// Provider kind identifier (e.g. `"anthropic"`, `"openai_compatible"`).
    pub kind: String,

    /// Model identifiers this provider supports.
    #[serde(default)]
    pub supported_models: Vec<String>,

    /// Whether this provider supports streaming responses.
    #[serde(default)]
    pub supports_streaming: bool,

    /// Whether this provider supports vision (image) inputs.
    #[serde(default)]
    pub supports_vision: bool,
}

/// Specification of a TTS provider (reserved for future use).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TtsProviderSpec {
    /// Provider kind identifier (e.g. `"openai_tts"`).
    pub kind: String,

    /// Voice identifiers this provider supports.
    #[serde(default)]
    pub supported_voices: Vec<String>,
}

/// Specification of an STT provider (reserved for future use).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SttProviderSpec {
    /// Provider kind identifier (e.g. `"whisper"`).
    pub kind: String,

    /// Language codes this provider supports (empty = auto-detect).
    #[serde(default)]
    pub supported_languages: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_default_is_empty() {
        let caps = PluginCapabilities::default();
        assert_eq!(caps.tools, 0);
        assert!(caps.llm_providers.is_empty());
        assert!(caps.tts_providers.is_empty());
        assert!(caps.stt_providers.is_empty());
    }

    #[test]
    fn capabilities_serde_roundtrip() {
        let caps = PluginCapabilities {
            tools: 0,
            llm_providers: vec![LlmProviderSpec {
                kind: "anthropic".into(),
                supported_models: vec!["claude-sonnet-4-20250514".into()],
                supports_streaming: true,
                supports_vision: true,
            }],
            tts_providers: vec![],
            stt_providers: vec![],
        };
        let json = serde_json::to_string(&caps).unwrap();
        let deser: PluginCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(caps, deser);
    }

    #[test]
    fn capabilities_deserialize_minimal() {
        let json = r"{}";
        let caps: PluginCapabilities = serde_json::from_str(json).unwrap();
        assert_eq!(caps, PluginCapabilities::default());
    }

    #[test]
    fn llm_provider_spec_serde_roundtrip() {
        let spec = LlmProviderSpec {
            kind: "anthropic".into(),
            supported_models: vec!["claude-sonnet-4-20250514".into(), "claude-haiku".into()],
            supports_streaming: true,
            supports_vision: false,
        };
        let json = serde_json::to_string(&spec).unwrap();
        let deser: LlmProviderSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, deser);
    }

    #[test]
    fn tts_provider_spec_serde_roundtrip() {
        let spec = TtsProviderSpec {
            kind: "openai_tts".into(),
            supported_voices: vec!["alloy".into(), "nova".into()],
        };
        let json = serde_json::to_string(&spec).unwrap();
        let deser: TtsProviderSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, deser);
    }

    #[test]
    fn stt_provider_spec_serde_roundtrip() {
        let spec = SttProviderSpec {
            kind: "whisper".into(),
            supported_languages: vec!["en".into(), "ja".into()],
        };
        let json = serde_json::to_string(&spec).unwrap();
        let deser: SttProviderSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, deser);
    }
}
