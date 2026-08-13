//! Plugin capability declarations.
//!
//! A plugin advertises its capabilities during the handshake so the host
//! can route tool registrations, LLM provider factories, and future
//! TTS/STT provider factories appropriately.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

/// Capabilities advertised by a plugin during the handshake.
///
/// The host inspects this struct after a successful `HandshakeAck` to
/// decide which registries to populate:
///
/// - `tools` → merged into the composite tool registry
/// - `llm_providers` → registered as `LlmProviderFactory` entries
/// - `tts_providers` / `stt_providers` → registered as TTS / STT provider
///   factories
/// - dynamic-config flags → gate `ListConfigOptions` / `ValidateConfig` /
///   `MigrateConfig` (protocol v5+) so older v5 binaries that lack those
///   variants are never sent them
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCapabilities {
    /// Number of tools this plugin provides (call `ListTools` for specs).
    #[serde(default)]
    pub tools: usize,

    /// LLM providers exposed by this plugin.
    #[serde(default)]
    pub llm_providers: Vec<LlmProviderSpec>,

    /// Provider kinds for which this plugin serves batch embeddings
    /// ([`crate::PluginIpcRequest::EmbedBatch`]).
    ///
    /// Unlike the provider traits above there is no per-kind embedding spec —
    /// the model and dimensions come from the host's configuration per
    /// request — so the capability is a bare kind list.
    #[serde(default)]
    pub embed_providers: Vec<String>,

    /// TTS providers served by this plugin (`SynthesizeSpeech`).
    #[serde(default)]
    pub tts_providers: Vec<TtsProviderSpec>,

    /// STT providers served by this plugin (`Transcribe`).
    #[serde(default)]
    pub stt_providers: Vec<SttProviderSpec>,

    /// VAD engines served by this plugin.
    ///
    /// Absent on older binaries (`#[serde(default)]` → empty); the host then
    /// registers no VAD factory for the plugin.
    #[serde(default)]
    pub vad_providers: Vec<VadProviderSpec>,

    /// Whether the plugin handles [`crate::PluginIpcRequest::ListConfigOptions`].
    ///
    /// Absent on older binaries (`#[serde(default)]` → `false`); the host
    /// then skips the IPC and degrades to static schema only.
    #[serde(default)]
    pub supports_list_config_options: bool,

    /// Whether the plugin handles [`crate::PluginIpcRequest::ValidateConfig`].
    ///
    /// Absent on older binaries (`#[serde(default)]` → `false`); the host
    /// then validates with JSON Schema locally instead of delegating.
    #[serde(default)]
    pub supports_validate_config: bool,

    /// Whether the plugin handles [`crate::PluginIpcRequest::MigrateConfig`].
    ///
    /// Absent on older binaries (`#[serde(default)]` → `false`); the host
    /// then skips migration.
    #[serde(default)]
    pub supports_migrate_config: bool,

    /// Current configuration schema version this plugin expects.
    ///
    /// `0` (default) means the plugin does not version its config. When the
    /// host's stored version is older, it may call `MigrateConfig` if
    /// [`supports_migrate_config`](Self::supports_migrate_config) is set.
    #[serde(default)]
    pub config_version: u32,

    /// Whether the plugin handles [`crate::PluginIpcRequest::CapabilityCall`].
    ///
    /// Absent on older binaries (`#[serde(default)]` → `false`); the host
    /// then refuses to mediate capability calls into this plugin — a binary
    /// that predates the call message cannot decode it, and a clean typed
    /// error beats a connection-level decode failure. A plugin must also
    /// declare the capability in [`provides`](Self::provides) for the host to
    /// route calls to it at all.
    #[serde(default)]
    pub supports_capability_calls: bool,

    /// Capabilities this plugin provides to other plugins, each written
    /// `name@major` (e.g. `gguf-runner@1`).
    ///
    /// Absent on older binaries (`#[serde(default)]` → empty); the host then
    /// indexes nothing for this plugin. The host validates each entry at
    /// registration and drops invalid ones individually with a warning —
    /// a bad string never fails the whole handshake.
    #[serde(default)]
    pub provides: Vec<CapabilityRef>,

    /// Capabilities this plugin requires from other plugins, each written
    /// `name@[^]major[?]` (e.g. `gguf-runner@^1`; a trailing `?` marks a
    /// soft requirement the plugin can fall back from).
    ///
    /// Absent on older binaries (`#[serde(default)]` → empty); the host then
    /// gates nothing for this plugin. Invalid entries are dropped
    /// individually with a warning (see [`CapabilityRef`]).
    #[serde(default)]
    pub requires: Vec<CapabilityRequirement>,
}

/// A capability reference: `name@major` (e.g. `gguf-runner@1`).
///
/// `name` is one or more lowercase segments of `[a-z0-9]` plus `-`, joined by
/// `/` (`llm/chat`, `g2p/ja`, `gguf-runner`). `major` is a decimal integer
/// without leading zeros. Versions beyond the major are deliberately absent
/// from the wire form: capability evolution policy (what a major means, when
/// it must change) lives in the host documentation, not in this type.
///
/// The serde form is transparent, so `provides: ["gguf-runner@1"]` on the
/// wire deserializes losslessly. Serde intentionally does **not** validate:
/// the host validates each declaration entry and drops invalid ones
/// individually (see [`PluginCapabilities::provides`]) so one typo cannot
/// fail a plugin's whole handshake. Use [`Self::parse`] for validated
/// construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityRef(String);

impl CapabilityRef {
    /// Parses and validates a `name@major` capability reference.
    ///
    /// # Errors
    /// Returns [`CapabilityParseError`] when the string is not a valid
    /// `name@major` reference.
    pub fn parse(raw: &str) -> Result<Self, CapabilityParseError> {
        let (name, major) = raw
            .split_once('@')
            .ok_or_else(|| CapabilityParseError::InvalidRef(raw.to_string()))?;
        if !is_valid_capability_name(name) {
            return Err(CapabilityParseError::InvalidName(name.to_string()));
        }
        if parse_major(major).is_none() {
            return Err(CapabilityParseError::InvalidMajor(major.to_string()));
        }
        Ok(Self(raw.to_string()))
    }

    /// Returns the raw `name@major` string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the capability name (the part before `@`).
    ///
    /// `None` only for a string that was deserialized without validation —
    /// values constructed via [`Self::parse`] always have one.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.0.split_once('@').map(|(name, _)| name)
    }

    /// Returns the major version (the part after `@`).
    ///
    /// `None` only for a string that was deserialized without validation —
    /// values constructed via [`Self::parse`] always have one.
    #[must_use]
    pub fn major(&self) -> Option<u32> {
        self.0
            .split_once('@')
            .and_then(|(_, major)| parse_major(major))
    }
}

impl FromStr for CapabilityRef {
    type Err = CapabilityParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<&str> for CapabilityRef {
    type Error = CapabilityParseError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::parse(s)
    }
}

impl fmt::Display for CapabilityRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A capability requirement: `name@[^]major[?]` (e.g. `gguf-runner@^1`).
///
/// The optional `^` prefix declares compatibility intent (`^1` = "any 1.x");
/// on today's wire — where the reference version is a bare major — it matches
/// the same set as `@1` and exists so consumers state their intent before
/// minor versions exist. The optional trailing `?` marks a **soft**
/// requirement: the plugin can start and degrade gracefully when no provider
/// is present. Without it the requirement is hard: the host disables the
/// plugin when no provider matches.
///
/// Like [`CapabilityRef`], serde is transparent and unvalidated; use
/// [`Self::parse`] for validated construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityRequirement(String);

impl CapabilityRequirement {
    /// Parses and validates a `name@[^]major[?]` capability requirement.
    ///
    /// # Errors
    /// Returns [`CapabilityParseError`] when the string is not a valid
    /// requirement.
    pub fn parse(raw: &str) -> Result<Self, CapabilityParseError> {
        let without_soft = raw.strip_suffix('?').unwrap_or(raw);
        let (name, rest) = without_soft
            .split_once('@')
            .ok_or_else(|| CapabilityParseError::InvalidRef(raw.to_string()))?;
        if !is_valid_capability_name(name) {
            return Err(CapabilityParseError::InvalidName(name.to_string()));
        }
        let major = rest.strip_prefix('^').unwrap_or(rest);
        if parse_major(major).is_none() {
            return Err(CapabilityParseError::InvalidMajor(major.to_string()));
        }
        Ok(Self(raw.to_string()))
    }

    /// Returns the raw `name@[^]major[?]` string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the capability name (the part before `@`).
    ///
    /// `None` only for a string that was deserialized without validation.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.0
            .strip_suffix('?')
            .unwrap_or(&self.0)
            .split_once('@')
            .map(|(name, _)| name)
    }

    /// Returns the required major version.
    ///
    /// `None` only for a string that was deserialized without validation.
    #[must_use]
    pub fn major(&self) -> Option<u32> {
        let without_soft = self.0.strip_suffix('?').unwrap_or(&self.0);
        without_soft
            .split_once('@')
            .and_then(|(_, rest)| parse_major(rest.strip_prefix('^').unwrap_or(rest)))
    }

    /// Returns whether the requirement is soft (`?` suffix) — the plugin may
    /// start and fall back when no provider matches.
    #[must_use]
    pub fn is_soft(&self) -> bool {
        self.0.ends_with('?')
    }

    /// Returns whether the requirement declared `^` compatibility intent.
    #[must_use]
    pub fn is_compatible(&self) -> bool {
        self.0
            .strip_suffix('?')
            .unwrap_or(&self.0)
            .split_once('@')
            .is_some_and(|(_, rest)| rest.starts_with('^'))
    }
}

impl FromStr for CapabilityRequirement {
    type Err = CapabilityParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<&str> for CapabilityRequirement {
    type Error = CapabilityParseError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::parse(s)
    }
}

impl fmt::Display for CapabilityRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Errors from parsing [`CapabilityRef`] / [`CapabilityRequirement`] strings.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CapabilityParseError {
    /// The string has no `@` separating name and version.
    #[error("capability must be `name@version`, got `{0}`")]
    InvalidRef(String),
    /// The name violates the `[a-z0-9-]` segment / `/`-joined shape.
    #[error("capability name `{0}` must be lowercase `[a-z0-9-]` segments joined by `/`")]
    InvalidName(String),
    /// The version is not a decimal integer without leading zeros.
    #[error("capability version `{0}` must be a non-negative integer without leading zeros")]
    InvalidMajor(String),
}

/// Validates a capability name: one or more `[a-z0-9]+(-[a-z0-9]+)*` segments
/// joined by single `/` separators. The charset deliberately excludes `.` and
/// `_` so capability namespaces stay free-form rather than colliding with
/// connector/credential id conventions.
fn is_valid_capability_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    name.split('/').all(is_valid_capability_segment)
}

fn is_valid_capability_segment(segment: &str) -> bool {
    let mut parts = segment.split('-');
    let Some(first) = parts.next() else {
        return false;
    };
    if first.is_empty()
        || !first
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return false;
    }
    parts.all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

/// Parses a decimal major version: digits only, no leading zeros.
fn parse_major(raw: &str) -> Option<u32> {
    if raw.is_empty()
        || !raw.bytes().all(|b| b.is_ascii_digit())
        || (raw.len() > 1 && raw.starts_with('0'))
    {
        return None;
    }
    raw.parse().ok()
}

/// Specification of an LLM provider exposed by a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmProviderSpec {
    /// Provider kind identifier (e.g. `"anthropic"`, `"openai"`).
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

    /// The physical resource this provider's jobs contend on, used by the
    /// host's admission control to share one budget across every provider
    /// that declares the same class (e.g. all GPU-offloaded local models on
    /// device 0). Absent (or omitted by an older plugin binary) defaults to
    /// [`ResourceClass::Cpu`] — the conservative "contends on CPU" reading —
    /// so old specs keep negotiating without a protocol version bump.
    #[serde(default)]
    pub resource_class: ResourceClass,
}

/// Specification of a TTS provider.
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

/// Specification of an STT provider.
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

/// Specification of a voice activity detection engine.
///
/// `frame_size` is the one piece of engine state the host must know
/// synchronously: a host-side `VadEngine` adapter has to answer
/// `frame_size()` without an IPC round trip, so it carries the value from
/// this spec. `sample_rate` is the PCM rate chunks arrive at; the host's
/// capture pipeline runs at 16 kHz today and reserves this field for
/// negotiating other rates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VadProviderSpec {
    /// Engine kind identifier (e.g. `"silero"`).
    pub kind: String,

    /// PCM samples per [`crate::PluginIpcRequest::ProcessVadChunk`] call.
    #[serde(default)]
    pub frame_size: u32,

    /// PCM sample rate the engine expects (Hz).
    ///
    /// Absent (or omitted by an older binary) defaults to
    /// [`DEFAULT_SAMPLE_RATE`] — 16 kHz, the rate every built-in VAD engine
    /// and the desktop capture pipeline use.
    #[serde(default = "default_vad_sample_rate")]
    pub sample_rate: u32,

    /// How many concurrent sessions this engine can safely serve.
    ///
    /// Absent (or omitted by an older plugin binary) defaults to
    /// [`ConcurrencyHint::default`] — serial, shallow queue. See that type's
    /// docs for the rationale.
    #[serde(default)]
    pub concurrency: ConcurrencyHint,
}

/// Default VAD sample rate: 16 kHz (Silero VAD's native rate, shared by the
/// desktop capture pipeline).
pub const DEFAULT_SAMPLE_RATE: u32 = 16_000;

fn default_vad_sample_rate() -> u32 {
    DEFAULT_SAMPLE_RATE
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
        assert!(caps.vad_providers.is_empty());
        assert!(!caps.supports_list_config_options);
        assert!(!caps.supports_validate_config);
        assert!(!caps.supports_migrate_config);
        assert_eq!(caps.config_version, 0);
        assert!(!caps.supports_capability_calls);
        assert!(caps.provides.is_empty());
        assert!(caps.requires.is_empty());
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
                resource_class: ResourceClass::Gpu { device: 0 },
            }],
            embed_providers: vec!["openai".into()],
            tts_providers: vec![],
            stt_providers: vec![],
            vad_providers: vec![],
            supports_list_config_options: true,
            supports_validate_config: true,
            supports_migrate_config: true,
            config_version: 2,
            supports_capability_calls: true,
            provides: vec![
                CapabilityRef::parse("llm/chat@1").unwrap(),
                CapabilityRef::parse("embed@1").unwrap(),
                CapabilityRef::parse("gguf-runner@1").unwrap(),
            ],
            requires: vec![CapabilityRequirement::parse("gguf-runner@^1").unwrap()],
        };
        let json = serde_json::to_string(&caps).unwrap();
        let deser: PluginCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(caps, deser);
    }

    #[test]
    fn capabilities_missing_dynamic_config_flags_default_false() {
        let json = r#"{"tools":1}"#;
        let caps: PluginCapabilities = serde_json::from_str(json).unwrap();
        assert!(!caps.supports_list_config_options);
        assert!(!caps.supports_validate_config);
        assert!(!caps.supports_migrate_config);
        assert_eq!(caps.config_version, 0);
        assert!(!caps.supports_capability_calls);
        assert!(caps.provides.is_empty());
        assert!(caps.requires.is_empty());
    }

    #[test]
    fn capabilities_deserialize_minimal() {
        let json = r"{}";
        let caps: PluginCapabilities = serde_json::from_str(json).unwrap();
        assert_eq!(caps, PluginCapabilities::default());
    }

    /// Load-bearing contract: a plugin binary that predates capability
    /// declarations omits `provides`/`requires` on the wire; they must
    /// deserialize as empty, not error — this is what lets the fields ship
    /// without a protocol version bump.
    #[test]
    fn capabilities_missing_declarations_default_empty() {
        let json = r#"{"tools":1}"#;
        let caps: PluginCapabilities = serde_json::from_str(json).unwrap();
        assert!(caps.provides.is_empty());
        assert!(caps.requires.is_empty());
    }

    #[test]
    fn capability_ref_parse_accepts_canonical_forms() {
        for raw in [
            "gguf-runner@1",
            "llm/chat@1",
            "embed@1",
            "g2p/ja@0",
            "a@4294967295",
        ] {
            let parsed = CapabilityRef::parse(raw).unwrap();
            assert_eq!(parsed.as_str(), raw);
            let (name, major) = raw.split_once('@').unwrap();
            assert_eq!(parsed.name(), Some(name));
            assert_eq!(parsed.major(), major.parse().ok());
        }
    }

    #[test]
    fn capability_ref_parse_rejects_malformed() {
        for raw in [
            "gguf-runner",
            "gguf-runner@",
            "@1",
            "gguf-runner@x",
            "gguf-runner@1.0",
            "gguf-runner@01",
            "gguf-runner@-1",
            "gguf-runner@1?",
            "gguf-runner@^1",
            "GGUF-runner@1",
            "gguf_runner@1",
            "gguf.runner@1",
            "/runner@1",
            "gguf//runner@1",
            "gguf-runner/@1",
            "-gguf@1",
            "gguf-@1",
            "gguf runner@1",
            "gguf-runner@1 ",
            "",
        ] {
            assert!(
                CapabilityRef::parse(raw).is_err(),
                "expected {raw:?} to be rejected"
            );
        }
    }

    #[test]
    fn capability_requirement_parse_accepts_canonical_forms() {
        for (raw, soft, compatible) in [
            ("gguf-runner@^1", false, true),
            ("gguf-runner@1", false, false),
            ("gguf-runner@1?", true, false),
            ("gguf-runner@^1?", true, true),
            ("g2p/ja@^2", false, true),
        ] {
            let parsed = CapabilityRequirement::parse(raw).unwrap();
            assert_eq!(parsed.as_str(), raw);
            assert_eq!(parsed.is_soft(), soft);
            assert_eq!(parsed.is_compatible(), compatible);
            let (name, major) = raw
                .strip_suffix('?')
                .unwrap_or(raw)
                .split_once('@')
                .unwrap();
            assert_eq!(parsed.name(), Some(name));
            assert_eq!(parsed.major(), major.trim_start_matches('^').parse().ok());
        }
    }

    #[test]
    fn capability_requirement_parse_rejects_malformed() {
        for raw in [
            "gguf-runner",
            "gguf-runner@",
            "gguf-runner@^",
            "gguf-runner@?1",
            "gguf-runner@1^",
            "gguf-runner@1?x",
            "gguf-runner@01",
            "gguf-runner@^01?",
            "gguf-runner@1..",
            "@^1",
            "gguf-runner@1? ",
            "",
        ] {
            assert!(
                CapabilityRequirement::parse(raw).is_err(),
                "expected {raw:?} to be rejected"
            );
        }
    }

    #[test]
    fn capability_ref_serde_is_transparent() {
        let json = r#""gguf-runner@1""#;
        let parsed: CapabilityRef = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.name(), Some("gguf-runner"));
        assert_eq!(parsed.major(), Some(1));
        assert_eq!(serde_json::to_string(&parsed).unwrap(), json);
    }

    #[test]
    fn capability_requirement_serde_is_transparent() {
        let json = r#""gguf-runner@^1?""#;
        let parsed: CapabilityRequirement = serde_json::from_str(json).unwrap();
        assert!(parsed.is_soft());
        assert!(parsed.is_compatible());
        assert_eq!(serde_json::to_string(&parsed).unwrap(), json);
    }

    #[test]
    fn capability_declarations_serde_roundtrip() {
        let caps = PluginCapabilities {
            provides: vec![CapabilityRef::parse("gguf-runner@1").unwrap()],
            requires: vec![
                CapabilityRequirement::parse("gguf-runner@^1").unwrap(),
                CapabilityRequirement::parse("g2p/ja@^1?").unwrap(),
            ],
            ..PluginCapabilities::default()
        };
        let json = serde_json::to_string(&caps).unwrap();
        let deser: PluginCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(caps, deser);
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
            resource_class: ResourceClass::Gpu { device: 0 },
        };
        let json = serde_json::to_string(&spec).unwrap();
        let deser: LlmProviderSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, deser);
        assert!(
            json.contains(r#"{"Gpu":{"device":0}}"#),
            "externally tagged ResourceClass form must be load-bearing on the wire"
        );
    }

    #[test]
    fn llm_provider_spec_defaults_resource_class_to_cpu() {
        // A spec produced by an older plugin binary omits `resource_class`
        // entirely; it must parse as `Cpu` without a protocol version bump.
        let json = r#"{
            "kind": "anthropic",
            "supported_models": [],
            "supports_streaming": true,
            "supports_vision": false,
            "concurrency": {"max_in_flight": 1, "queue_depth": 2}
        }"#;
        let spec: LlmProviderSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.resource_class, ResourceClass::Cpu);
        assert_eq!(ResourceClass::default(), ResourceClass::Cpu);
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

    #[test]
    fn vad_provider_spec_serde_roundtrip() {
        let spec = VadProviderSpec {
            kind: "silero".into(),
            frame_size: 512,
            sample_rate: 16_000,
            concurrency: ConcurrencyHint::default(),
        };
        let json = serde_json::to_string(&spec).unwrap();
        let deser: VadProviderSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, deser);
    }

    /// Load-bearing contract: an absent `frame_size` (as an old binary that
    /// predates the field would send) deserializes to 0 rather than an
    /// error, so the host can reject it explicitly.
    #[test]
    fn vad_provider_spec_missing_frame_size_defaults_to_zero() {
        let json = r#"{"kind":"silero"}"#;
        let spec: VadProviderSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.frame_size, 0);
        assert_eq!(spec.sample_rate, DEFAULT_SAMPLE_RATE);
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

/// The physical resource a provider's jobs contend on, and the key an
/// admission budget uses to share one semaphore across engines that contend
/// on the same one.
///
/// Two engines that declare `==` `ResourceClass` values are treated as
/// contending on the same physical device or capacity and share one budget:
/// adding a new local model that offloads to the same GPU device (for
/// example) automatically starts sharing that device's budget the moment it
/// declares [`ResourceClass::Gpu`] with the same device index — no change to
/// callers or to engines already using that class.
///
/// `Cpu` deliberately carries no field — a distinguishing number would make a
/// capacity question (how many CPU-bound jobs may run at once) masquerade as
/// an identity question (are these the same resource). Every CPU-bound engine
/// declares the same `Cpu` value and shares one process-wide budget; how
/// large that budget is (whether two independent CPU-bound engines may run
/// concurrently at all) is an admission-layer decision, not part of this
/// type.
///
/// Wire note: carried on `LlmProviderSpec.resource_class` in the externally
/// tagged serde form (`"Cpu"` / `{"Gpu":{"device":0}}` / `"Network"`), which
/// the host's per-class admission budgets key on; the form is pinned by the
/// serde tests below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub enum ResourceClass {
    /// A specific GPU device index, as used by `with_main_gpu(n)` /
    /// CUDA/Vulkan device selection. A real, meaningful identity: two
    /// engines that declare the same device index genuinely do contend on
    /// that one physical device.
    Gpu {
        /// Device index.
        device: u32,
    },
    /// CPU-bound inference. Shared by every CPU-bound engine — see the
    /// type-level docs for why this carries no field.
    Cpu,
    /// A network-attached engine (e.g. a local sidecar process reached over
    /// HTTP/gRPC) that does not contend on host GPU/CPU capacity the same
    /// way.
    Network,
}

impl Default for ResourceClass {
    /// The conservative default for an undeclared provider: CPU-bound
    /// inference, which every provider can fall back to.
    fn default() -> Self {
        Self::Cpu
    }
}

#[cfg(test)]
mod resource_class_tests {
    use super::*;

    #[test]
    fn resource_class_serde_roundtrip() {
        for class in [
            ResourceClass::Gpu { device: 0 },
            ResourceClass::Gpu { device: 3 },
            ResourceClass::Cpu,
            ResourceClass::Network,
        ] {
            let json = serde_json::to_string(&class).unwrap();
            let deser: ResourceClass = serde_json::from_str(&json).unwrap();
            assert_eq!(class, deser);
        }
    }

    #[test]
    fn resource_class_default_representation() {
        assert_eq!(
            serde_json::to_string(&ResourceClass::Cpu).unwrap(),
            r#""Cpu""#
        );
        assert_eq!(
            serde_json::to_string(&ResourceClass::Gpu { device: 0 }).unwrap(),
            r#"{"Gpu":{"device":0}}"#
        );
        assert_eq!(
            serde_json::to_string(&ResourceClass::Network).unwrap(),
            r#""Network""#
        );
    }

    #[test]
    fn resource_class_equality_and_hash_are_by_value() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let hash_of = |class: ResourceClass| {
            let mut hasher = DefaultHasher::new();
            class.hash(&mut hasher);
            hasher.finish()
        };

        // Equal values compare equal and hash equal — the contract
        // `HashMap<ResourceClass, _>` admission budgets rely on.
        assert_eq!(
            ResourceClass::Gpu { device: 0 },
            ResourceClass::Gpu { device: 0 }
        );
        assert_eq!(
            hash_of(ResourceClass::Gpu { device: 0 }),
            hash_of(ResourceClass::Gpu { device: 0 })
        );
        // Distinct device indices are distinct resources.
        assert_ne!(
            ResourceClass::Gpu { device: 0 },
            ResourceClass::Gpu { device: 1 }
        );
        // `Cpu` carries no field: every `Cpu` is the one shared value.
        assert_eq!(ResourceClass::Cpu, ResourceClass::Cpu);
        assert_ne!(ResourceClass::Cpu, ResourceClass::Gpu { device: 0 });
    }
}
