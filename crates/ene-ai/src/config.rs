const fn default_string() -> String {
    String::new()
}

use std::collections::BTreeMap;

use ene_config::schemars;

/// Reserved task provider name that resolves against [`AiConfig::local_models`].
pub const LOCAL_PROVIDER: &str = "local";

/// Provider kind served by the OpenAI-compatible provider plugin
/// (`plugins/provider/openai`).
pub const OPENAI_PROVIDER_KIND: &str = "openai";

/// Default API base for OpenAI-compatible requests when neither `base_url`
/// nor `OPENAI_BASE_URL` is configured.
///
/// Mirrors the `OpenAI` provider plugin's own default so pre-plugin resolution
/// paths (embedding setup, health probes) agree with where the plugin will
/// actually send requests.
pub const DEFAULT_OPENAI_API_BASE: &str = "https://api.openai.com/v1";

/// Legacy `kind` value for OpenAI-compatible providers, accepted as an alias
/// for [`OPENAI_PROVIDER_KIND`] so pre-plugin configs keep working. The
/// canonical registry kind is always [`OPENAI_PROVIDER_KIND`].
pub const LEGACY_OPENAI_COMPATIBLE_KIND: &str = "openai_compatible";

/// The canonical provider kind for `kind`, folding the legacy
/// `openai_compatible` alias onto the `openai` plugin kind.
///
/// Registry lookups and factory def-matching must go through this mapping so
/// both spellings resolve to the same plugin-backed provider.
#[must_use]
pub fn canonical_provider_kind(kind: &str) -> &str {
    if kind == LEGACY_OPENAI_COMPATIBLE_KIND {
        OPENAI_PROVIDER_KIND
    } else {
        kind
    }
}

/// Well-known provider [`AiProviderDef::kind`] values.
///
/// Provider backends ship as plugins; this list only exists to catch typos
/// of the well-known kinds (see [`kind_typo_suggestion`]). Any other kind is
/// assumed to name a plugin-provided backend and is validated leniently (see
/// [`crate::resolve::validate_provider_kinds`]).
pub const BUILTIN_PROVIDER_KINDS: &[&str] = &[
    OPENAI_PROVIDER_KIND,
    LEGACY_OPENAI_COMPATIBLE_KIND,
    "anthropic",
];

ene_config::define_config!(
    AiConfig,
    "api_key",
    /// Configuration for API key retrieval.
    pub struct ApiKeyConfig {
        /// Key source: `"inline"` or `"env"`.
        pub source: String = "env".to_string(),
        /// API key (inline — use with caution).
        pub inline: String = default_string(),
        /// Environment variable name when `source = "env"`.
        pub env: String = "OPENAI_API_KEY".to_string(),
    }
);

// Manual PartialEq: all fields compared; kept explicit to ensure future fields
// (e.g. secrets) are consciously included rather than auto-derived.
impl PartialEq for ApiKeyConfig {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source && self.inline == other.inline && self.env == other.env
    }
}

ene_config::define_label_enum!(
    /// GPU / CPU acceleration preference for local llama.cpp.
    pub enum ProactiveAcceleration {
        /// Pick from OS / available backends / startup result.
        #[default]
        Auto => "auto",
        /// Force Vulkan (AMD RADV / cross-vendor Vulkan).
        Vulkan => "vulkan",
        /// Force CUDA.
        Cuda => "cuda",
        /// CPU only.
        Cpu => "cpu",
    }
);

/// Cloud / plugin provider definition.
///
/// `kind` selects the backend (`"openai"`, `"anthropic"`, …; the legacy
/// `"openai_compatible"` spelling is an alias of `"openai"`).
/// OpenAI-compatible providers use the typed [`AiProviderDef::base_url`] and
/// [`AiProviderDef::api_key`] fields; provider-specific options for other
/// kinds are captured in the [`AiProviderDef::extra`] catch-all. The struct
/// form is backward-compatible with the previous tagged-enum JSON:
/// `{"kind": "openai", "base_url": "...", "api_key": {...}}`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct AiProviderDef {
    /// Provider backend kind (e.g. `"openai"`, `"anthropic"`; the legacy
    /// `"openai_compatible"` spelling is accepted as an alias of `"openai"`).
    pub kind: String,
    /// API base URL (empty → `OPENAI_BASE_URL` env, then the `OpenAI` plugin
    /// default, for `openai_compatible`).
    #[serde(default = "default_string")]
    pub base_url: String,
    /// API key configuration.
    #[serde(default)]
    pub api_key: ApiKeyConfig,
    /// Explicit context-window override in tokens.
    ///
    /// Caps the window a provider advertises (`LlmProviderSpec.context_window`,
    /// or `LocalModelDef.context_size` for local models): the effective window
    /// is `min(advertised, this)`, so an operator can run a large-window model
    /// on a smaller budget but never exceed the model's stated limit. `None`
    /// (the default) defers entirely to whatever the provider advertises.
    #[serde(default)]
    pub context_window: Option<u32>,
    /// Provider-specific options not covered by the typed fields.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl AiProviderDef {
    /// True when this definition targets an OpenAI-compatible HTTP provider
    /// (kind `"openai"`, or the legacy `"openai_compatible"` alias).
    #[must_use]
    pub fn is_openai_compatible(&self) -> bool {
        matches!(
            self.kind.as_str(),
            OPENAI_PROVIDER_KIND | LEGACY_OPENAI_COMPATIBLE_KIND
        )
    }
}

impl Default for AiProviderDef {
    fn default() -> Self {
        Self {
            kind: "openai_compatible".to_string(),
            base_url: default_string(),
            api_key: ApiKeyConfig::default(),
            context_window: None,
            extra: serde_json::Map::new(),
        }
    }
}

/// True when `kind` is a built-in provider backend recognized without a
/// plugin (one of [`BUILTIN_PROVIDER_KINDS`]).
#[must_use]
pub fn is_builtin_kind(kind: &str) -> bool {
    BUILTIN_PROVIDER_KINDS.contains(&kind)
}

/// Suggests a built-in kind when `kind` looks like a typo of one.
///
/// Returns the closest built-in kind when the edit distance is small enough to
/// indicate a likely typo (e.g. `"openai_compatable"` → `"openai_compatible"`,
/// `"openai-compatible"` → `"openai_compatible"`, `"anthroic"` →
/// `"anthropic"`). Returns `None` when `kind` is already built-in or too
/// different to be a plausible typo, so genuinely plugin-provided kinds are
/// left untouched.
#[must_use]
pub fn kind_typo_suggestion(kind: &str) -> Option<&'static str> {
    if is_builtin_kind(kind) {
        return None;
    }
    let mut best: Option<(&'static str, usize)> = None;
    for &candidate in BUILTIN_PROVIDER_KINDS {
        let distance = levenshtein(kind, candidate);
        let is_better = best.is_none_or(|(_, best_distance)| distance < best_distance);
        if distance <= typo_tolerance(kind, candidate) && is_better {
            best = Some((candidate, distance));
        }
    }
    best.map(|(candidate, _)| candidate)
}

/// Maximum edit distance still considered a plausible typo for a given pair of
/// kind strings. Short kinds must match almost exactly; longer kinds tolerate
/// a couple of stray characters.
fn typo_tolerance(kind: &str, candidate: &str) -> usize {
    let max_len = kind.len().max(candidate.len());
    if max_len <= 8 { 1 } else { 2 }
}

/// Classic character-based Levenshtein edit distance.
///
/// Only ever called on short provider-kind strings, so the quadratic DP cost
/// is negligible. Indexing and integer arithmetic are bounds-safe by
/// construction (indices derive from the row/column lengths).
#[expect(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "bounded Levenshtein DP: indices derive from row/column lengths and counts stay within small kind strings"
)]
fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();
    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }
    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0_usize; b_len + 1];
    for i in 1..=a_len {
        curr[0] = i;
        for j in 1..=b_len {
            let cost = usize::from(a_chars[i - 1] != b_chars[j - 1]);
            let deletion = prev[j] + 1;
            let insertion = curr[j - 1] + 1;
            let substitution = prev[j - 1] + cost;
            curr[j] = deletion.min(insertion).min(substitution);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b_len]
}

/// Named local GGUF model entry under [`AiConfig::local_models`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct LocalModelDef {
    /// HTTPS URL for the GGUF weights (downloaded on first use).
    pub url: String,
    /// Quantization label (e.g. `"F16"`, `"Q4_0"`).
    #[serde(default = "default_local_quantization")]
    pub quantization: String,
    /// Explicit filesystem path override (skips download when non-empty).
    #[serde(default = "default_string")]
    pub model_path: String,
    /// GPU layer offload: `"auto"` or an integer string (e.g. `"33"`).
    // TODO: Replace `String` with an `AutoOrU32` enum to catch misconfiguration
    // at the config boundary instead of silently falling back at load time.
    #[serde(default = "default_gpu_layers")]
    pub gpu_layers: String,
    /// Context window for this model, in tokens.
    ///
    /// Sized to hold the full main-conversation prompt when the model backs a
    /// `tasks.chat` / `tasks.proactive` workload (see [`default_context_size`]
    /// for the rationale and VRAM trade-off). A model used only for small
    /// decision tasks (classification, proactive triggers) can lower this to
    /// shrink its KV cache.
    #[serde(default = "default_context_size")]
    pub context_size: u32,
    /// Real embedding vector dimensionality, in f32 components.
    ///
    /// Required when the model backs `tasks.embedding` with
    /// `provider: "local"`: the host opens the memory-store vec0 schema with
    /// this number before the plugin host starts (the plugin measures the
    /// model's real `n_embd` only at load time, which is too late for the
    /// store schema), and the plugin rejects a declared value that differs
    /// from the model's real dimensionality. Ignored for chat-only models.
    #[serde(default)]
    pub dimensions: Option<usize>,
}

impl Default for LocalModelDef {
    fn default() -> Self {
        Self {
            url: default_string(),
            quantization: default_local_quantization(),
            model_path: default_string(),
            gpu_layers: default_gpu_layers(),
            context_size: default_context_size(),
            dimensions: None,
        }
    }
}

fn default_local_quantization() -> String {
    "F16".to_string()
}

fn default_gpu_layers() -> String {
    "auto".to_string()
}

/// Default context window for a local model, in tokens.
///
/// Calibrated to hold the full main-conversation prompt, which needs on the
/// order of 12,000 tokens when a local model backs `tasks.chat`, with room
/// left for the model's reply. The prompt auto-follows the model's window —
/// no separate prompt-token budget caps it — so a window sized only for small
/// decision tasks (classification, proactive triggers) silently drops most
/// prompt sections; 16,384 is a comfortable floor for a full conversation
/// prompt plus reply.
///
/// 16,384 is deliberately chosen over 32,768: llama.cpp's KV cache grows
/// linearly with context length, so for a `Gemma-3-4B`-class model (34 layers,
/// GQA with 4 KV heads, `head_dim` 256, fp16) the cache is roughly 2.3 GB at
/// 16K versus ~4.6 GB at 32K — on top of the model weights — which does not
/// fit many environments. A model used only for decision workloads can set a
/// smaller `context_size` explicitly to shrink its KV cache; the startup
/// validation ([`crate::resolve::validate_context_budgets`]) warns whenever a
/// configured window is too small for the prompt budget plus output reserve.
const fn default_context_size() -> u32 {
    16_384
}

/// Task routing: which provider and model to use for a cognitive workload.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct TaskRef {
    /// Named entry in [`AiConfig::providers`] or [`LOCAL_PROVIDER`].
    pub provider: String,
    /// Cloud model name, or a key in [`AiConfig::local_models`] when `provider` is [`LOCAL_PROVIDER`].
    pub model: Option<String>,
    /// Max completion tokens for chat workloads.
    pub max_tokens: Option<u32>,
    /// Expected embedding dimensions (cloud workloads).
    pub dimensions: Option<usize>,
    /// Optional query prefix for embedding retrieval queries (e.g. `"Query: "`).
    pub query_prefix: Option<String>,
    /// When true, proactive generation may attach the decision-time screen frame.
    #[serde(default)]
    pub supports_vision: bool,
    /// When true, the provider sends `thinking: {type: disabled}` for this
    /// task's requests.
    ///
    /// The runtime sets this for the decision classifier: it is a short
    /// structured-output call, and reasoning models that emit their answer in
    /// `reasoning_content` (MiMo-class and beyond the plugin's name heuristic)
    /// otherwise stall or corrupt the JSON reply.
    #[serde(default)]
    pub thinking_disabled: bool,
}

impl Default for TaskRef {
    fn default() -> Self {
        Self {
            provider: "default".to_string(),
            model: None,
            max_tokens: None,
            dimensions: None,
            query_prefix: None,
            supports_vision: false,
            thinking_disabled: false,
        }
    }
}

/// Per-task AI routing under [`AiConfig::tasks`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct AiTasksConfig {
    /// Main conversation chat model.
    pub chat: TaskRef,
    /// Embedding model.
    pub embedding: TaskRef,
    /// Optional lightweight classifier (falls back to chat).
    pub classifier: Option<TaskRef>,
    /// Optional proactive speech generation override (falls back to chat).
    pub proactive: Option<TaskRef>,
}

impl Default for AiTasksConfig {
    fn default() -> Self {
        Self {
            chat: TaskRef {
                provider: "default".to_string(),
                model: Some("gpt-4o-mini".to_string()),
                max_tokens: Some(8192),
                dimensions: None,
                query_prefix: None,
                supports_vision: false,
                thinking_disabled: false,
            },
            embedding: TaskRef {
                provider: "default".to_string(),
                model: Some("text-embedding-3-small".to_string()),
                max_tokens: None,
                dimensions: Some(1536),
                query_prefix: None,
                supports_vision: false,
                thinking_disabled: false,
            },
            classifier: None,
            proactive: None,
        }
    }
}

fn default_providers() -> BTreeMap<String, AiProviderDef> {
    let mut providers = BTreeMap::new();
    providers.insert("default".to_string(), AiProviderDef::default());
    providers
}

/// TTS (text-to-speech) provider configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct TtsConfig {
    /// Provider name (`"none"` disables TTS).
    pub provider: String,
    /// Model name (provider-specific).
    pub model: String,
    /// Voice identifier (provider-specific, e.g. `"af_heart"`).
    pub voice: String,
    /// Speech speed multiplier (1.0 = normal).
    pub speed: f32,
    /// Language code for grapheme-to-phoneme conversion (e.g. `"en"`, `"ja"`;
    /// empty defaults to English).
    pub language: String,
    /// Explicit filesystem path to the TTS model (used when non-empty).
    pub model_path: Option<String>,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            provider: "none".to_string(),
            model: String::new(),
            voice: String::new(),
            speed: 1.0,
            language: String::new(),
            model_path: None,
        }
    }
}

/// STT (speech-to-text) provider configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct SttConfig {
    /// Provider name (`"none"` disables STT).
    pub provider: String,
    /// Model name (provider-specific).
    pub model: String,
    /// Language hint (e.g. `"ja"`, `"en"`; empty = auto-detect).
    pub language: String,
    /// Explicit filesystem path to the STT model (used when non-empty).
    pub model_path: Option<String>,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            provider: "none".to_string(),
            model: String::new(),
            language: String::new(),
            model_path: None,
        }
    }
}

/// VAD (voice activity detection) engine configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct VadConfig {
    /// Engine name (`"none"` disables VAD).
    pub provider: String,
    /// Model name (provider-specific).
    pub model: String,
    /// Speech probability threshold (0.0–1.0).
    pub threshold: f32,
    /// Explicit filesystem path to the VAD model (used when non-empty).
    pub model_path: Option<String>,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            provider: "none".to_string(),
            model: String::new(),
            threshold: 0.5,
            model_path: None,
        }
    }
}

/// Retry / backoff policy for transient provider failures.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct RetryConfig {
    /// Maximum number of attempts (including the initial call).
    pub max_attempts: u32,
    /// Base delay before the first retry, in milliseconds.
    pub base_delay_ms: u64,
    /// Maximum delay cap for exponential growth, in milliseconds.
    pub max_delay_ms: u64,
    /// Per-request timeout, in milliseconds.
    pub timeout_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay_ms: 500,
            max_delay_ms: 30_000,
            timeout_ms: 120_000,
        }
    }
}

impl RetryConfig {
    /// Convert to the runtime [`crate::retry::RetryPolicy`].
    #[must_use]
    pub fn to_policy(&self) -> crate::retry::RetryPolicy {
        crate::retry::RetryPolicy {
            max_attempts: self.max_attempts.max(1),
            base_delay: std::time::Duration::from_millis(self.base_delay_ms),
            max_delay: std::time::Duration::from_millis(self.max_delay_ms),
            timeout: std::time::Duration::from_millis(self.timeout_ms),
        }
    }
}

/// Provider health-check and failover policy.
///
/// When `enabled`, the runtime probes each configured cloud provider's
/// `/models` endpoint (with a timeout, sending no user data) and selects the
/// first healthy provider in priority order for chat workloads. If the
/// primary provider is unhealthy, the runtime falls back to the next
/// available provider and records the event for diagnostics.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct FallbackConfig {
    /// Enable provider health checks and automatic failover.
    pub enabled: bool,
    /// Per-probe timeout, in milliseconds.
    pub health_check_timeout_ms: u64,
    /// How long a cached health result is considered fresh, in milliseconds.
    pub cache_ttl_ms: u64,
    /// Maximum number of fallback events retained for diagnostics.
    pub max_history: usize,
}

impl Default for FallbackConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            health_check_timeout_ms: 5_000,
            cache_ttl_ms: 60_000,
            max_history: 32,
        }
    }
}

ene_config::define_config!(
    settings,
    "ai",
    /// AI provider registry and per-task routing.
    pub struct AiConfig {
        /// Named local GGUF model registry.
        pub local_models: BTreeMap<String, LocalModelDef> = BTreeMap::new(),
        /// Named cloud provider definitions.
        pub providers: BTreeMap<String, AiProviderDef> = default_providers(),
        /// Task → provider/model routing.
        pub tasks: AiTasksConfig,
        /// Retry / backoff policy for transient provider failures.
        pub retry: RetryConfig,
/// Provider health-check and failover policy.
        pub fallback: FallbackConfig,
        /// TTS (text-to-speech) provider settings.
        pub tts: TtsConfig,
        /// STT (speech-to-text) provider settings.
        pub stt: SttConfig,
        /// VAD (voice activity detection) engine settings.
        pub vad: VadConfig,
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{ResolvedEmbedding, resolve_base_url};

    fn test_config() -> AiConfig {
        let mut cfg = AiConfig::default();
        if let Some(def) = cfg.providers.get_mut("default") {
            def.base_url = "https://api.openai.com/v1".to_string();
        }
        cfg
    }

    #[test]
    fn ai_config_defaults() {
        let cfg = AiConfig::default();
        assert!(cfg.local_models.is_empty());
        assert_eq!(cfg.providers.len(), 1);
        let default = cfg.providers.get("default").expect("default provider");
        assert!(default.is_openai_compatible());
        assert!(default.base_url.is_empty());
        assert_eq!(cfg.tasks.chat.model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(cfg.tasks.chat.max_tokens, Some(8192));
        assert_eq!(
            cfg.tasks.embedding.model.as_deref(),
            Some("text-embedding-3-small")
        );
        assert_eq!(cfg.tasks.embedding.dimensions, Some(1536));
        assert!(cfg.tasks.classifier.is_none());
        assert!(cfg.tasks.proactive.is_none());
        assert_eq!(ApiKeyConfig::default().source, "env");
    }

    #[test]
    fn task_ref_thinking_disabled_defaults_false_and_round_trips() {
        let task: TaskRef =
            serde_json::from_str(r#"{"provider": "default", "model": "gpt-4o-mini"}"#)
                .expect("pre-field config files must deserialize");
        assert!(!task.thinking_disabled);

        let task: TaskRef =
            serde_json::from_str(r#"{"provider": "default", "thinking_disabled": true}"#)
                .expect("deserialize with thinking_disabled");
        assert!(task.thinking_disabled);
        assert_eq!(
            serde_json::to_value(&task)
                .expect("serialize")
                .get("thinking_disabled"),
            Some(&serde_json::json!(true))
        );
    }

    #[test]
    fn local_model_default_context_size_is_16k() {
        // The default must hold the 12,000-token prompt budget plus a
        // response reserve.
        assert_eq!(default_context_size(), 16_384);
        assert_eq!(LocalModelDef::default().context_size, 16_384);
    }

    #[test]
    fn resolve_embedding_honors_query_prefix() {
        let mut cfg = test_config();
        cfg.tasks.embedding.query_prefix = Some("Query: ".to_string());
        let embed = cfg.resolve_embedding().expect("resolve embedding");
        match embed {
            ResolvedEmbedding::Cloud { query_prefix, .. } => {
                assert_eq!(query_prefix.as_deref(), Some("Query: "));
            }
            ResolvedEmbedding::Local(_) => panic!("expected cloud embedding"),
        }
    }

    #[test]
    fn resolve_chat_and_embedding() {
        let cfg = test_config();
        let chat = cfg.resolve_chat().expect("resolve chat");
        assert_eq!(chat.model, "gpt-4o-mini");
        assert_eq!(chat.max_tokens, Some(8192));

        let embed = cfg.resolve_embedding().expect("resolve embedding");
        match embed {
            ResolvedEmbedding::Cloud {
                model, dimensions, ..
            } => {
                assert_eq!(model, "text-embedding-3-small");
                assert_eq!(dimensions, 1536);
            }
            ResolvedEmbedding::Local(_) => panic!("expected cloud embedding"),
        }
    }

    #[test]
    fn resolve_classifier_falls_back_to_chat() {
        let cfg = test_config();
        let classifier = cfg.resolve_classifier().expect("resolve classifier");
        let chat = cfg.resolve_chat().expect("resolve chat");
        assert_eq!(classifier.model, chat.model);
        assert_eq!(classifier.base_url, chat.base_url);
    }

    #[test]
    fn two_providers_different_base_url() {
        let mut cfg = AiConfig::default();
        cfg.providers.insert(
            "alt".to_string(),
            AiProviderDef {
                base_url: "https://api.example.com/v1".to_string(),
                ..AiProviderDef::default()
            },
        );
        cfg.tasks.chat.provider = "alt".to_string();
        let chat = cfg.resolve_chat().expect("resolve chat");
        assert_eq!(chat.base_url, "https://api.example.com/v1");
    }

    #[test]
    fn resolve_base_url_from_explicit() {
        let url = resolve_base_url("https://custom.example/v1").expect("url");
        assert_eq!(url, "https://custom.example/v1");
    }

    #[test]
    fn local_task_resolves_from_local_models() {
        let mut cfg = test_config();
        cfg.local_models.insert(
            "jina-v5-small".to_string(),
            LocalModelDef {
                url: "https://example.com/v5-small.gguf".to_string(),
                quantization: "F16".to_string(),
                ..LocalModelDef::default()
            },
        );
        cfg.tasks.embedding = TaskRef {
            provider: LOCAL_PROVIDER.to_string(),
            model: Some("jina-v5-small".to_string()),
            max_tokens: None,
            dimensions: None,
            query_prefix: None,
            supports_vision: false,
            thinking_disabled: false,
        };
        let embed = cfg.resolve_embedding().expect("resolve local embedding");
        match embed {
            ResolvedEmbedding::Local(local) => {
                assert_eq!(local.name, "jina-v5-small");
                assert_eq!(local.quantization, "F16");
                assert_eq!(local.url, "https://example.com/v5-small.gguf");
            }
            ResolvedEmbedding::Cloud { .. } => panic!("expected local embedding"),
        }
    }

    #[test]
    fn ai_provider_def_deserializes_tagged() {
        let def: AiProviderDef = serde_json::from_str(
            r#"{"kind":"openai_compatible","base_url":"https://api.example.com/v1","api_key":{"source":"env","env":"OPENAI_API_KEY","inline":""}}"#,
        )
        .expect("deserialize");
        assert!(def.is_openai_compatible());
        assert_eq!(def.base_url, "https://api.example.com/v1");
        assert!(def.extra.is_empty());
    }

    #[test]
    fn ai_provider_def_captures_unknown_kind_extra() {
        let def: AiProviderDef = serde_json::from_str(
            r#"{"kind":"anthropic","api_key":{"source":"env","env":"ANTHROPIC_API_KEY","inline":""},"region":"us-east-1"}"#,
        )
        .expect("deserialize");
        assert!(!def.is_openai_compatible());
        assert_eq!(def.kind, "anthropic");
        assert!(def.base_url.is_empty());
        assert_eq!(def.api_key.env, "ANTHROPIC_API_KEY");
        assert_eq!(
            def.extra.get("region").and_then(serde_json::Value::as_str),
            Some("us-east-1")
        );
    }

    #[test]
    fn ai_provider_def_round_trips_extra() {
        let mut extra = serde_json::Map::new();
        extra.insert(
            "region".to_string(),
            serde_json::Value::String("us-east-1".to_string()),
        );
        let def = AiProviderDef {
            kind: "anthropic".to_string(),
            extra,
            ..AiProviderDef::default()
        };
        let json = serde_json::to_value(&def).expect("serialize");
        assert_eq!(json.get("kind").expect("kind field"), "anthropic");
        assert_eq!(json.get("region").expect("region field"), "us-east-1");
        let back: AiProviderDef = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, def);
    }

    #[test]
    fn builtin_kind_recognizes_known_kinds() {
        assert!(is_builtin_kind("openai_compatible"));
        assert!(is_builtin_kind("anthropic"));
        assert!(!is_builtin_kind("my-plugin"));
        assert!(!is_builtin_kind(""));
    }

    #[test]
    fn kind_typo_suggestion_catches_near_misses() {
        assert_eq!(
            kind_typo_suggestion("openai-compatible"),
            Some("openai_compatible")
        );
        assert_eq!(
            kind_typo_suggestion("openai_compatable"),
            Some("openai_compatible")
        );
        assert_eq!(kind_typo_suggestion("anthroic"), Some("anthropic"));
    }

    #[test]
    fn kind_typo_suggestion_ignores_builtin_and_plugin_kinds() {
        assert_eq!(kind_typo_suggestion("openai_compatible"), None);
        assert_eq!(kind_typo_suggestion("anthropic"), None);
        assert_eq!(kind_typo_suggestion("my-custom-plugin"), None);
        assert_eq!(kind_typo_suggestion("gemini"), None);
    }

    #[test]
    fn levenshtein_basic_distances() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("anthropic", "anthroic"), 1);
    }
}
