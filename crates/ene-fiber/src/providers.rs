//! In-tree provider plugin ids, binaries, and declared seams.

use std::path::Path;

use serde_json::{Value, json};

use crate::spawn::discover_plugin_executable_in;

/// One bundled provider plugin.
#[derive(Debug, Clone, Copy)]
pub struct ProviderPlugin {
    pub id: &'static str,
    pub bin: &'static str,
    pub seams: &'static [&'static str],
    /// Offers a GGUF / sidecar path (the catalog `provider.gguf` row).
    pub local: bool,
    /// Desktop should collect a vault API key for this plugin.
    pub needs_key: bool,
}

/// Providers shipped next to `ene-core`.
///
/// Desktop and `ai.tasks.*` pickers list this table — not a parallel UI
/// allowlist. Adding a provider plugin means adding a row here.
pub const PROVIDER_PLUGINS: &[ProviderPlugin] = &[
    ProviderPlugin {
        id: "provider.gguf",
        bin: "ene-provider-gguf",
        seams: &["seam.llm", "seam.embed"],
        local: true,
        needs_key: false,
    },
    ProviderPlugin {
        id: "provider.openai_compat",
        bin: "ene-provider-openai-compat",
        seams: &["seam.llm", "seam.embed", "seam.tts", "seam.stt"],
        local: false,
        needs_key: true,
    },
    ProviderPlugin {
        id: "provider.anthropic",
        bin: "ene-provider-anthropic",
        seams: &["seam.llm"],
        local: false,
        needs_key: true,
    },
    ProviderPlugin {
        id: "provider.elevenlabs",
        bin: "ene-provider-elevenlabs",
        seams: &["seam.tts"],
        local: false,
        needs_key: true,
    },
    ProviderPlugin {
        id: "provider.voicevox",
        bin: "ene-provider-voicevox",
        seams: &["seam.tts"],
        local: true,
        needs_key: false,
    },
    ProviderPlugin {
        id: "provider.edge_tts",
        bin: "ene-provider-edge-tts",
        seams: &["seam.tts"],
        local: false,
        needs_key: false,
    },
];

#[must_use]
pub fn provider_plugin(id: &str) -> Option<&'static ProviderPlugin> {
    PROVIDER_PLUGINS.iter().find(|plugin| plugin.id == id)
}

/// Seam required by an `ai.tasks.*` row.
#[must_use]
pub fn task_seam(task: &str) -> Option<&'static str> {
    match task {
        "chat" | "classifier" | "proactive" => Some("seam.llm"),
        "embedding" => Some("seam.embed"),
        "tts" => Some("seam.tts"),
        "stt" => Some("seam.stt"),
        _ => None,
    }
}

/// Catalog with `installed` reflecting a binary on the plugin search path.
#[must_use]
pub fn provider_catalog(home: Option<&Path>) -> Vec<Value> {
    PROVIDER_PLUGINS
        .iter()
        .map(|plugin| {
            json!({
                "id": plugin.id,
                "seams": plugin.seams,
                "local": plugin.local,
                "needs_key": plugin.needs_key,
                "installed": discover_plugin_executable_in(plugin.id, home).is_some(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gguf_is_the_local_llm_and_embed_provider() {
        let plugin = provider_plugin("provider.gguf").expect("bundled");
        assert!(plugin.local);
        assert!(!plugin.needs_key);
        assert_eq!(plugin.bin, "ene-provider-gguf");
        assert!(plugin.seams.contains(&"seam.llm"));
        assert!(plugin.seams.contains(&"seam.embed"));
    }

    #[test]
    fn openai_compat_is_cloud_only() {
        let plugin = provider_plugin("provider.openai_compat").expect("bundled");
        assert!(!plugin.local);
        assert!(plugin.needs_key);
        assert!(plugin.seams.contains(&"seam.llm"));
        assert!(plugin.seams.contains(&"seam.embed"));
    }

    #[test]
    fn task_seams_cover_ai_tasks() {
        assert_eq!(task_seam("chat"), Some("seam.llm"));
        assert_eq!(task_seam("embedding"), Some("seam.embed"));
        assert_eq!(task_seam("tts"), Some("seam.tts"));
        assert_eq!(task_seam("unknown"), None);
    }
}
