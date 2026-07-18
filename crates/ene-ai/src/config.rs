const fn default_string() -> String {
    String::new()
}

use std::collections::BTreeMap;

use ene_config::schemars;

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

/// Provider definition: cloud OpenAI-compatible API or local GGUF weights.
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
    /// Local GGUF model via llama-cpp-2 (embedding and/or proactive decision).
    LocalGguf {
        /// Hub model name for embedding (e.g. `"jina-embeddings-v5-text-small"`).
        #[serde(default = "default_local_gguf_model")]
        model: String,
        /// Quantization level (e.g. `"F16"`, `"Q4_K_M"`).
        #[serde(default = "default_local_gguf_quantization")]
        quantization: String,
        /// Filesystem path for local decision LLM GGUF (empty = embedding-only / HF Hub).
        #[serde(default = "default_string")]
        model_path: String,
        /// Preferred acceleration backend.
        #[serde(default)]
        acceleration: ProactiveAcceleration,
        /// GPU layer offload: `"auto"` or an integer string (e.g. `"33"`).
        #[serde(default = "default_gpu_layers")]
        gpu_layers: String,
        /// Context size for the decision model (small is preferred).
        #[serde(default = "default_context_size")]
        context_size: u32,
    },
}

fn default_local_gguf_model() -> String {
    "jina-embeddings-v5-text-small".to_string()
}

fn default_local_gguf_quantization() -> String {
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
    /// Named entry in [`AiConfig::providers`].
    pub provider: String,
    /// Model override (empty / absent → provider default or error at resolve time).
    pub model: Option<String>,
    /// Max completion tokens for chat workloads.
    pub max_tokens: Option<u32>,
    /// Expected embedding dimensions (cloud workloads).
    pub dimensions: Option<usize>,
}

impl Default for TaskRef {
    fn default() -> Self {
        Self {
            provider: "default".to_string(),
            model: None,
            max_tokens: None,
            dimensions: None,
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
            },
            embedding: TaskRef {
                provider: "default".to_string(),
                model: Some("text-embedding-3-small".to_string()),
                max_tokens: None,
                dimensions: Some(1536),
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
        /// Named provider definitions.
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
            ResolvedEmbedding::Local { .. } => panic!("expected cloud embedding"),
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
    fn ai_provider_def_deserializes_tagged() {
        let def: AiProviderDef =
            serde_json::from_str(r#"{"kind":"local_gguf","model_path":"/tmp/m.gguf"}"#)
                .expect("deserialize");
        match def {
            AiProviderDef::LocalGguf { model_path, .. } => {
                assert_eq!(model_path, "/tmp/m.gguf");
            }
            AiProviderDef::OpenaiCompatible { .. } => panic!("expected local_gguf"),
        }
    }
}
