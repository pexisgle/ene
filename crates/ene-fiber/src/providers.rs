//! In-tree provider plugin ids, binaries, and declared seams.

/// One bundled provider plugin.
#[derive(Debug, Clone, Copy)]
pub struct ProviderPlugin {
    pub id: &'static str,
    pub bin: &'static str,
    pub seams: &'static [&'static str],
}

/// Providers shipped next to `ene-core`.
pub const PROVIDER_PLUGINS: &[ProviderPlugin] = &[
    ProviderPlugin {
        id: "provider.openai_compat",
        bin: "ene-provider-openai-compat",
        seams: &["seam.llm", "seam.embed", "seam.tts", "seam.stt"],
    },
    ProviderPlugin {
        id: "provider.anthropic",
        bin: "ene-provider-anthropic",
        seams: &["seam.llm"],
    },
    ProviderPlugin {
        id: "provider.elevenlabs",
        bin: "ene-provider-elevenlabs",
        seams: &["seam.tts"],
    },
    ProviderPlugin {
        id: "provider.voicevox",
        bin: "ene-provider-voicevox",
        seams: &["seam.tts"],
    },
    ProviderPlugin {
        id: "provider.edge_tts",
        bin: "ene-provider-edge-tts",
        seams: &["seam.tts"],
    },
];

#[must_use]
pub fn provider_plugin(id: &str) -> Option<&'static ProviderPlugin> {
    PROVIDER_PLUGINS.iter().find(|plugin| plugin.id == id)
}
