//! Static catalog for `provider.gguf` (weights + llama-server sidecar).

use ene_provider_assets::{AssetCatalog, AssetKind, CatalogAsset, CatalogVersion, PlatformTarget};

pub const PLUGIN_ID: &str = "provider.gguf";

const LLAMA_WINDOWS: CatalogVersion = CatalogVersion {
    version: "b4282",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b4282/llama-b4282-bin-win-cpu-x64.zip",
    sha256: "",
    size_bytes: None,
    filename: "llama-server.exe",
    platform: Some(PlatformTarget {
        os: "windows",
        arch: "x86_64",
    }),
    recommended: true,
    archive_member: Some("llama-server.exe"),
};

const LLAMA_LINUX: CatalogVersion = CatalogVersion {
    version: "b4282",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b4282/llama-b4282-bin-ubuntu-x64.zip",
    sha256: "",
    size_bytes: None,
    filename: "llama-server",
    platform: Some(PlatformTarget {
        os: "linux",
        arch: "x86_64",
    }),
    recommended: true,
    archive_member: Some("llama-server"),
};

const SIDECAR_VERSIONS: &[CatalogVersion] = &[LLAMA_WINDOWS, LLAMA_LINUX];

const CATALOG_ROWS: &[CatalogAsset] = &[
    CatalogAsset {
        id: "llama-server",
        kind: AssetKind::Sidecar,
        label: "llama-server",
        description: "Local GGUF inference engine",
        recommended: true,
        seams: &[],
        versions: SIDECAR_VERSIONS,
    },
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
