//! Curated GGUF catalog and host-side HTTPS download for local weights.
//!
//! Plugins must never download weights; the desktop host owns this path.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

/// Chat vs embedding GGUF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeightKind {
    Chat,
    Embedding,
}

/// One curated GGUF offered on the AI page.
#[derive(Clone, Copy, Debug)]
pub struct CatalogEntry {
    /// Value written to `ai.tasks.*.model`.
    pub id: &'static str,
    pub filename: &'static str,
    pub url: &'static str,
    pub kind: WeightKind,
    pub recommended: bool,
}

/// Static allowlist. Download URLs outside this set are rejected.
pub const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        id: "gemma-4-e2b",
        filename: "gemma-4-E2B-it-Q4_0.gguf",
        url: "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-Q4_0.gguf",
        kind: WeightKind::Chat,
        recommended: true,
    },
    CatalogEntry {
        id: "gemma-4-e4b",
        filename: "gemma-4-E4B-it-Q4_0.gguf",
        url: "https://huggingface.co/unsloth/gemma-4-E4B-it-GGUF/resolve/main/gemma-4-E4B-it-Q4_0.gguf",
        kind: WeightKind::Chat,
        recommended: false,
    },
    CatalogEntry {
        id: "jina-v5-small",
        filename: "v5-small-retrieval-F16.gguf",
        url: "https://huggingface.co/jinaai/jina-embeddings-v5-text-small-retrieval/resolve/main/v5-small-retrieval-F16.gguf",
        kind: WeightKind::Embedding,
        recommended: true,
    },
];

#[must_use]
pub fn entry_by_id(id: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|entry| entry.id == id)
}

#[must_use]
pub fn recommended_chat() -> &'static CatalogEntry {
    CATALOG
        .iter()
        .find(|entry| entry.kind == WeightKind::Chat && entry.recommended)
        .unwrap_or(&CATALOG[0])
}

#[must_use]
pub fn recommended_embedding() -> &'static CatalogEntry {
    CATALOG
        .iter()
        .find(|entry| entry.kind == WeightKind::Embedding && entry.recommended)
        .or_else(|| {
            CATALOG
                .iter()
                .find(|entry| entry.kind == WeightKind::Embedding)
        })
        .unwrap_or(&CATALOG[0])
}

pub fn chat_entries() -> impl Iterator<Item = &'static CatalogEntry> {
    CATALOG
        .iter()
        .filter(|entry| entry.kind == WeightKind::Chat)
}

pub fn embed_entries() -> impl Iterator<Item = &'static CatalogEntry> {
    CATALOG
        .iter()
        .filter(|entry| entry.kind == WeightKind::Embedding)
}

#[must_use]
pub fn gguf_dir() -> PathBuf {
    ene_config::models_dir().join("gguf")
}

#[must_use]
pub fn catalog_dest(entry: &CatalogEntry) -> PathBuf {
    gguf_dir().join(entry.filename)
}

#[must_use]
pub fn is_downloaded(entry: &CatalogEntry) -> bool {
    catalog_dest(entry).is_file()
}

#[must_use]
pub fn is_allowlisted_url(url: &str) -> bool {
    url.starts_with("https://") && CATALOG.iter().any(|entry| entry.url == url)
}

/// Whether `name` resolves as an executable on `PATH`.
#[must_use]
pub fn binary_on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return true;
        }
        #[cfg(target_os = "windows")]
        {
            let with_exe = dir.join(format!("{name}.exe"));
            if with_exe.is_file() {
                return true;
            }
        }
        false
    })
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DownloadProgress {
    pub received: u64,
    pub total: Option<u64>,
}

/// In-flight or finished GGUF download tracked by the AI page.
#[derive(Debug, Default)]
pub struct GgufDownloadUi {
    entry_id: Option<String>,
    progress: Arc<Mutex<DownloadProgress>>,
    receiver: Option<oneshot::Receiver<Result<PathBuf, String>>>,
    pub last_error: Option<String>,
    /// Set when a download completes so the page can apply the binding once.
    pub completed_path: Option<(String, PathBuf, String)>,
    apply_task: Option<String>,
}

impl GgufDownloadUi {
    #[must_use]
    pub fn busy(&self) -> bool {
        self.receiver.is_some()
    }

    #[must_use]
    pub fn entry_id(&self) -> Option<&str> {
        self.entry_id.as_deref()
    }

    #[must_use]
    pub fn progress_snapshot(&self) -> DownloadProgress {
        *self.progress.lock()
    }

    pub fn start(&mut self, runtime: &tokio::runtime::Handle, entry: &CatalogEntry, task: &str) {
        if self.busy() {
            return;
        }
        if !is_allowlisted_url(entry.url) {
            self.last_error = Some("download url is not on the catalog allowlist".to_owned());
            return;
        }
        self.last_error = None;
        self.completed_path = None;
        self.apply_task = Some(task.to_owned());
        self.entry_id = Some(entry.id.to_owned());
        self.progress = Arc::new(Mutex::new(DownloadProgress::default()));
        let progress = Arc::clone(&self.progress);
        let entry = *entry;
        let (tx, rx) = oneshot::channel();
        self.receiver = Some(rx);
        runtime.spawn(async move {
            let result = download_catalog_entry(&entry, progress).await;
            drop(tx.send(result));
        });
    }

    pub fn poll(&mut self) {
        let Some(receiver) = self.receiver.as_mut() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(path)) => {
                let id = self.entry_id.clone().unwrap_or_default();
                let task = self.apply_task.clone().unwrap_or_else(|| "chat".to_owned());
                self.completed_path = Some((id, path, task));
                self.receiver = None;
            }
            Ok(Err(err)) => {
                self.last_error = Some(err);
                self.receiver = None;
                self.entry_id = None;
            }
            Err(oneshot::error::TryRecvError::Empty) => {}
            Err(oneshot::error::TryRecvError::Closed) => {
                self.last_error = Some("download cancelled".to_owned());
                self.receiver = None;
                self.entry_id = None;
            }
        }
    }
}

async fn download_catalog_entry(
    entry: &CatalogEntry,
    progress: Arc<Mutex<DownloadProgress>>,
) -> Result<PathBuf, String> {
    if !is_allowlisted_url(entry.url) {
        return Err("download url is not on the catalog allowlist".to_owned());
    }
    let dest = catalog_dest(entry);
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|err| format!("create model dir: {err}"))?;
    let partial = dest.with_extension("gguf.partial");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(8))
        .build()
        .map_err(|err| format!("http client: {err}"))?;
    let response = client
        .get(entry.url)
        .send()
        .await
        .map_err(|err| format!("download request: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("download failed: HTTP {}", response.status()));
    }
    let total = response.content_length();
    {
        let mut slot = progress.lock();
        slot.total = total;
        slot.received = 0;
    }
    let mut file = tokio::fs::File::create(&partial)
        .await
        .map_err(|err| format!("create partial file: {err}"))?;
    let mut stream = response;
    loop {
        let chunk = stream
            .chunk()
            .await
            .map_err(|err| format!("download stream: {err}"))?;
        let Some(bytes) = chunk else {
            break;
        };
        file.write_all(&bytes)
            .await
            .map_err(|err| format!("write model: {err}"))?;
        progress.lock().received += bytes.len() as u64;
    }
    file.flush()
        .await
        .map_err(|err| format!("flush model: {err}"))?;
    drop(file);
    tokio::fs::rename(&partial, &dest)
        .await
        .map_err(|err| format!("finalize model: {err}"))?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_urls_are_https_and_unique() {
        let mut urls = Vec::new();
        for entry in CATALOG {
            assert!(entry.url.starts_with("https://"));
            assert!(is_allowlisted_url(entry.url));
            assert!(!urls.contains(&entry.url));
            urls.push(entry.url);
        }
    }

    #[test]
    fn rejects_non_catalog_urls() {
        assert!(!is_allowlisted_url("https://example.invalid/model.gguf"));
        assert!(!is_allowlisted_url(
            "http://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-Q4_0.gguf"
        ));
    }

    #[test]
    fn recommended_chat_is_e2b() {
        assert_eq!(recommended_chat().id, "gemma-4-e2b");
    }

    #[test]
    fn recommended_embedding_is_jina() {
        assert_eq!(recommended_embedding().id, "jina-v5-small");
        assert_eq!(recommended_embedding().kind, WeightKind::Embedding);
    }
}
