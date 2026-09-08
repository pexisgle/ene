//! Static catalog for `provider.gguf` (weights only; llama-server from GitHub).

use ene_provider_assets::{AssetCatalog, AssetKind, CatalogAsset, CatalogVersion};

pub const PLUGIN_ID: &str = "provider.gguf";

const CATALOG_ROWS: &[CatalogAsset] = &[
    CatalogAsset {
        id: "gemma-4-e2b",
        kind: AssetKind::Weight,
        label: "Gemma 4 E2B (Q4)",
        description: "Recommended chat model",
        recommended: true,
        seams: &["seam.llm"],
        versions: &[CatalogVersion {
            version: "default",
            url: "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-Q4_0.gguf",
            sha256: "",
            size_bytes: None,
            filename: "gemma-4-E2B-it-Q4_0.gguf",
            platform: None,
            recommended: true,
            archive_member: None,
        }],
    },
    CatalogAsset {
        id: "gemma-4-e4b",
        kind: AssetKind::Weight,
        label: "Gemma 4 E4B (Q4)",
        description: "Larger chat model",
        recommended: false,
        seams: &["seam.llm"],
        versions: &[CatalogVersion {
            version: "default",
            url: "https://huggingface.co/unsloth/gemma-4-E4B-it-GGUF/resolve/main/gemma-4-E4B-it-Q4_0.gguf",
            sha256: "",
            size_bytes: None,
            filename: "gemma-4-E4B-it-Q4_0.gguf",
            platform: None,
            recommended: false,
            archive_member: None,
        }],
    },
    CatalogAsset {
        id: "jina-v5-small",
        kind: AssetKind::Weight,
        label: "Jina v5 small",
        description: "Recommended embedding model",
        recommended: true,
        seams: &["seam.embed"],
        versions: &[CatalogVersion {
            version: "default",
            url: "https://huggingface.co/jinaai/jina-embeddings-v5-text-small-retrieval/resolve/main/v5-small-retrieval-F16.gguf",
            sha256: "",
            size_bytes: None,
            filename: "v5-small-retrieval-F16.gguf",
            platform: None,
            recommended: true,
            archive_member: None,
        }],
    },
];

pub static CATALOG: AssetCatalog = AssetCatalog::new(CATALOG_ROWS);

#[must_use]
pub fn sidecar_binary_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}
