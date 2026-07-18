const fn default_string() -> String {
    String::new()
}

use std::collections::BTreeMap;

use ene_config::schemars;

/// Reserved task provider name that resolves against [`AiConfig::local_models`].
pub const LOCAL_PROVIDER: &str = "local";

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

impl PartialEq for ApiKeyConfig {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source && self.inline == other.inline && self.env == other.env
    }
}

ene_config::define_label_enum!(
    /// GPU / CPU acceleration preference for local llama.cpp (#165).
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

/// OpenAI-compatible cloud provider definition.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", tag = "kind", rename_all = "snake_case")]
#[schemars(crate = "::ene_config::schemars")]
pub enum AiProviderDef {
    /// OpenAI-compatible HTTP API (chat, embedding, cloud decision).
    OpenaiCompatible {
        /// API base URL (empty → `OPENAI_BASE_URL` env).
        #[serde(default = "default_string")]
        base_url: String,
        /// API key configuration.
        api_key: ApiKeyConfig,
    },
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
    /// Preferred acceleration backend.
    #[serde(default)]
    pub acceleration: ProactiveAcceleration,
    /// GPU layer offload: `"auto"` or an integer string (e.g. `"33"`).
    #[serde(default = "default_gpu_layers")]
    pub gpu_layers: String,
    /// Context size for decision workloads.
    #[serde(default = "default_context_size")]
    pub context_size: u32,
}

impl Default for LocalModelDef {
    fn default() -> Self {
        Self {
            url: default_string(),
            quantization: default_local_quantization(),
            model_path: default_string(),
            acceleration: ProactiveAcceleration::default(),
            gpu_layers: default_gpu_layers(),
            context_size: default_context_size(),
        }
    }
}

fn default_local_quantization() -> String {
    "F16".to_string()
}

fn default_gpu_layers() -> String {
    "auto".to_string()
}

const fn default_context_size() -> u32 {
    2048
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
}

impl Default for TaskRef {
    fn default() -> Self {
        Self {
            provider: "default".to_string(),
            model: None,
            max_tokens: None,
            dimensions: None,
            query_prefix: None,
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
            },
            embedding: TaskRef {
                provider: "default".to_string(),
                model: Some("text-embedding-3-small".to_string()),
                max_tokens: None,
                dimensions: Some(1536),
                query_prefix: None,
            },
            classifier: None,
            proactive: None,
        }
    }
}

fn default_providers() -> BTreeMap<String, AiProviderDef> {
    let mut providers = BTreeMap::new();
    providers.insert(
        "default".to_string(),
        AiProviderDef::OpenaiCompatible {
            base_url: String::new(),
            api_key: ApiKeyConfig::default(),
        },
    );
    providers
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
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{ResolvedEmbedding, resolve_base_url};

    fn test_config() -> AiConfig {
        let mut cfg = AiConfig::default();
        if let Some(AiProviderDef::OpenaiCompatible { base_url, .. }) =
            cfg.providers.get_mut("default")
        {
            *base_url = "https://api.openai.com/v1".to_string();
        }
        cfg
    }

    #[test]
    fn ai_config_defaults() {
        let cfg = AiConfig::default();
        assert!(cfg.local_models.is_empty());
        assert_eq!(cfg.providers.len(), 1);
        assert!(matches!(
            cfg.providers.get("default"),
            Some(AiProviderDef::OpenaiCompatible { .. })
        ));
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
            AiProviderDef::OpenaiCompatible {
                base_url: "https://api.example.com/v1".to_string(),
                api_key: ApiKeyConfig::default(),
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
        match def {
            AiProviderDef::OpenaiCompatible { base_url, .. } => {
                assert_eq!(base_url, "https://api.example.com/v1");
            }
        }
    }
}
