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

    /// How many concurrent jobs this provider can safely run.
    ///
    /// Absent (or omitted by an older plugin binary) defaults to
    /// [`ConcurrencyHint::default`] — serial, shallow queue. See that type's
    /// docs for the rationale.
    #[serde(default)]
    pub concurrency: ConcurrencyHint,

    /// Maximum context window in tokens, if the provider knows it.
    ///
    /// This is the model's hard limit on `prompt + completion` tokens. The
    /// host combines it with any user-configured override to derive the
    /// effective window it budgets prompts against. `None` means
    /// the provider does not advertise a limit — either because it genuinely
    /// has none, or because an older plugin binary predates this field and
    /// omitted it on the wire (`#[serde(default)]` keeps that a `None`
    /// rather than a deserialization error, so no protocol version bump is
    /// required).
    #[serde(default)]
    pub context_window: Option<u32>,
}

/// Specification of a TTS provider (reserved for future use).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TtsProviderSpec {
    /// Provider kind identifier (e.g. `"openai_tts"`, `"voicevox"`).
    pub kind: String,

    /// Supported voice names.
    #[serde(default)]
    pub voices: Vec<String>,

    /// Supported audio formats (e.g. `"wav"`, `"mp3"`, `"ogg"`).
    #[serde(default)]
    pub formats: Vec<String>,

    /// How many concurrent jobs this provider can safely run.
    ///
    /// Absent (or omitted by an older plugin binary) defaults to
    /// [`ConcurrencyHint::default`] — serial, shallow queue. See that type's
    /// docs for the rationale.
    #[serde(default)]
    pub concurrency: ConcurrencyHint,
}

/// Specification of an STT provider (reserved for future use).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SttProviderSpec {
    /// Provider kind identifier (e.g. `"whisper"`, `"openai_stt"`).
    pub kind: String,

    /// Supported model identifiers.
    #[serde(default)]
    pub models: Vec<String>,

    /// Supported audio formats (e.g. `"wav"`, `"mp3"`, `"ogg"`).
    #[serde(default)]
    pub formats: Vec<String>,

    /// How many concurrent jobs this provider can safely run.
    ///
    /// Absent (or omitted by an older plugin binary) defaults to
    /// [`ConcurrencyHint::default`] — serial, shallow queue. See that type's
    /// docs for the rationale.
    #[serde(default)]
    pub concurrency: ConcurrencyHint,
}

/// How many concurrent jobs a plugin-supplied provider (LLM, TTS, STT) can
/// safely accept.
///
/// ## Why the default is serial
///
/// A plugin binary is a separate out-of-process program. The process
/// boundary protects the *host* from a misbehaving plugin — a bad plugin
/// cannot exhaust the host's tokio blocking pool — but nothing protects the
/// *plugin itself* from the host opening unbounded concurrent requests
/// against it. A local-inference plugin (llama.cpp, whisper.cpp, a local
/// TTS engine) typically owns one model instance and can only run one job
/// at a time without corrupting shared state or thrashing.
///
/// `ConcurrencyHint::default()` is therefore **`max_in_flight: 1`,
/// `queue_depth: 2`** — serial execution with a shallow queue — rather than
/// anything permissive. This is a deliberate design decision: a plugin
/// author who has not thought about concurrency at all gets conservative,
/// safe behavior *because* they did not think about it. A plugin that wants
/// higher concurrency (e.g. a stateless HTTP proxy to a cloud API) must set
/// `concurrency` explicitly, and doing so is itself evidence the author
/// considered the question.
///
/// Older plugin binaries that predate this field simply omit it on the
/// wire; `#[serde(default)]` on the containing spec (and on `Default` for
/// this type) means they still negotiate normally and receive the same safe
/// serial default — no protocol version bump required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConcurrencyHint {
    /// Max jobs this provider can run at once.
    pub max_in_flight: u32,
    /// Extra jobs to queue before rejecting.
    pub queue_depth: u32,
}

impl Default for ConcurrencyHint {
    /// Serial execution, shallow queue: `max_in_flight: 1, queue_depth: 2`.
    ///
    /// See the type-level docs for why this conservative default — not a
    /// permissive one — is the load-bearing choice here.
    fn default() -> Self {
        Self {
            max_in_flight: 1,
            queue_depth: 2,
        }
    }
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
                concurrency: ConcurrencyHint {
                    max_in_flight: 4,
                    queue_depth: 8,
                },
                context_window: Some(200_000),
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
            concurrency: ConcurrencyHint {
                max_in_flight: 4,
                queue_depth: 8,
            },
            context_window: Some(200_000),
        };
        let json = serde_json::to_string(&spec).unwrap();
        let deser: LlmProviderSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, deser);
    }

    #[test]
    fn tts_provider_spec_serde_roundtrip() {
        let spec = TtsProviderSpec {
            kind: "openai_tts".into(),
            voices: vec!["alloy".into(), "nova".into()],
            formats: vec!["wav".into(), "mp3".into()],
            concurrency: ConcurrencyHint::default(),
        };
        let json = serde_json::to_string(&spec).unwrap();
        let deser: TtsProviderSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, deser);
    }

    #[test]
    fn stt_provider_spec_serde_roundtrip() {
        let spec = SttProviderSpec {
            kind: "whisper".into(),
            models: vec!["whisper-1".into(), "large-v3".into()],
            formats: vec!["wav".into(), "ogg".into()],
            concurrency: ConcurrencyHint::default(),
        };
        let json = serde_json::to_string(&spec).unwrap();
        let deser: SttProviderSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, deser);
    }

    /// Load-bearing contract: an unset `concurrency` field (as an old plugin
    /// binary that omits it on the wire would send) must deserialize to the
    /// serial default, not an error and not a permissive default.
    #[test]
    fn llm_provider_spec_missing_concurrency_defaults_to_serial() {
        let json = r#"{"kind":"anthropic","supported_models":[],"supports_streaming":true,"supports_vision":false}"#;
        let spec: LlmProviderSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.concurrency, ConcurrencyHint::default());
        assert_eq!(spec.concurrency.max_in_flight, 1);
        assert_eq!(spec.concurrency.queue_depth, 2);
    }

    /// Load-bearing contract: an unset `context_window` field (as an old
    /// plugin binary that predates the field would send) must deserialize to
    /// `None` — "provider does not advertise a limit" — not an error. This is
    /// what lets the field ship without a protocol version bump.
    #[test]
    fn llm_provider_spec_missing_context_window_defaults_to_none() {
        let json = r#"{"kind":"anthropic","supported_models":[],"supports_streaming":true,"supports_vision":false}"#;
        let spec: LlmProviderSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.context_window, None);
    }

    #[test]
    fn llm_provider_spec_context_window_roundtrips() {
        let json = r#"{"kind":"anthropic","supported_models":[],"supports_streaming":true,"supports_vision":false,"context_window":200000}"#;
        let spec: LlmProviderSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.context_window, Some(200_000));
    }

    #[test]
    fn tts_provider_spec_missing_concurrency_defaults_to_serial() {
        let json = r#"{"kind":"voicevox","voices":[],"formats":[]}"#;
        let spec: TtsProviderSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.concurrency, ConcurrencyHint::default());
    }

    #[test]
    fn stt_provider_spec_missing_concurrency_defaults_to_serial() {
        let json = r#"{"kind":"whisper","models":[],"formats":[]}"#;
        let spec: SttProviderSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.concurrency, ConcurrencyHint::default());
    }

    #[test]
    fn concurrency_hint_default_is_serial() {
        let hint = ConcurrencyHint::default();
        assert_eq!(hint.max_in_flight, 1);
        assert_eq!(hint.queue_depth, 2);
    }

    #[test]
    fn concurrency_hint_serde_roundtrip() {
        let hint = ConcurrencyHint {
            max_in_flight: 3,
            queue_depth: 5,
        };
        let json = serde_json::to_string(&hint).unwrap();
        let deser: ConcurrencyHint = serde_json::from_str(&json).unwrap();
        assert_eq!(hint, deser);
    }
}
