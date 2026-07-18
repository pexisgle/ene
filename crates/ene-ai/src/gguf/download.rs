//! Streaming GGUF download with in-place terminal progress (TTY) or throttled logs.

use crate::error::LlmProviderError;
use std::collections::{BTreeMap, HashMap};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex as AsyncMutex;

const PROGRESS_BAR_WIDTH: usize = 10;
const UPDATE_INTERVAL: Duration = Duration::from_millis(500);
/// Hard cap on a single GGUF download (30 GiB).
const MAX_DOWNLOAD_BYTES: u64 = 30 * 1024 * 1024 * 1024;
const GGUF_MAGIC: &[u8; 4] = b"GGUF";
const CACHE_HASH_HEX_LEN: usize = 12;

type InflightSlot = std::sync::Arc<AsyncMutex<Option<Result<(), String>>>>;

static HTTP_CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
static IN_FLIGHT: LazyLock<AsyncMutex<HashMap<String, InflightSlot>>> =
    LazyLock::new(|| AsyncMutex::new(HashMap::new()));
static PROGRESS: Mutex<ProgressState> = Mutex::new(ProgressState::new());

struct DownloadEntry {
    downloaded: u64,
    total: Option<u64>,
    last_update: Instant,
    last_pct_byte: u8,
}

struct ProgressState {
    tty: Option<bool>,
    active: BTreeMap<String, DownloadEntry>,
    rendered_lines: usize,
    last_log: Option<(String, Instant, u8)>,
}

impl ProgressState {
    const fn new() -> Self {
        Self {
            tty: None,
            active: BTreeMap::new(),
            rendered_lines: 0,
            last_log: None,
        }
    }

    fn is_tty(&mut self) -> bool {
        *self
            .tty
            .get_or_insert_with(|| std::io::stderr().is_terminal())
    }
}

fn http_client() -> Result<&'static reqwest::Client, LlmProviderError> {
    HTTP_CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_hours(1))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| LlmProviderError::Provider(format!("HTTP client init failed: {e}")))
}

fn format_size(bytes: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= GB {
        format!("{:.1} GB", bytes as f64 / GB)
    } else {
        format!("{:.1} MB", bytes as f64 / MB)
    }
}

fn progress_bar(pct: f64) -> String {
    let filled = ((pct / 100.0) * PROGRESS_BAR_WIDTH as f64).round() as usize;
    let filled = filled.min(PROGRESS_BAR_WIDTH);
    let empty = PROGRESS_BAR_WIDTH.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

fn pct_of(downloaded: u64, total: Option<u64>) -> f64 {
    total.map_or(0.0, |t| {
        if t == 0 {
            0.0
        } else {
            (downloaded as f64 / t as f64) * 100.0
        }
    })
}

fn format_progress_line(filename: &str, downloaded: u64, total: Option<u64>) -> String {
    let pct = pct_of(downloaded, total);
    let bar = progress_bar(pct);
    match total {
        Some(total_bytes) => format!(
            "[GgufDownload] {filename} {bar} {pct:>3.0}% {}/{}",
            format_size(downloaded),
            format_size(total_bytes),
        ),
        None => format!(
            "[GgufDownload] {filename} {bar} {pct:>3.0}% {}",
            format_size(downloaded),
        ),
    }
}

fn should_update(entry: &DownloadEntry, force: bool, pct_byte: u8) -> bool {
    force
        || entry.last_update.elapsed() >= UPDATE_INTERVAL
        || pct_byte.saturating_sub(entry.last_pct_byte) >= 1
}

fn render_tty(state: &mut ProgressState) {
    let lines: Vec<String> = state
        .active
        .iter()
        .map(|(name, entry)| format_progress_line(name, entry.downloaded, entry.total))
        .collect();
    let slot_count = state.rendered_lines.max(lines.len());

    let mut stderr = std::io::stderr().lock();
    if state.rendered_lines > 0 {
        let _ = write!(stderr, "\x1b[{}A", state.rendered_lines);
    }

    for i in 0..slot_count {
        let _ = write!(stderr, "\x1b[2K\r");
        if let Some(line) = lines.get(i) {
            let _ = writeln!(stderr, "{line}");
        } else {
            let _ = writeln!(stderr);
        }
    }

    state.rendered_lines = lines.len();
    let _ = stderr.flush();
}

fn log_throttled(state: &mut ProgressState, filename: &str, downloaded: u64, total: Option<u64>) {
    let pct_byte = pct_of(downloaded, total) as u8;
    let should_log = match state.last_log.as_ref() {
        Some((last_name, last_at, last_pct)) => {
            last_name != filename
                || last_at.elapsed() >= UPDATE_INTERVAL
                || pct_byte.saturating_sub(*last_pct) >= 1
        }
        None => true,
    };
    if !should_log {
        return;
    }
    tracing::info!("{}", format_progress_line(filename, downloaded, total));
    state.last_log = Some((filename.to_string(), Instant::now(), pct_byte));
}

fn report_progress(filename: &str, downloaded: u64, total: Option<u64>, force: bool) {
    let pct_byte = pct_of(downloaded, total) as u8;
    let mut state = PROGRESS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let entry = state
        .active
        .entry(filename.to_string())
        .or_insert_with(|| DownloadEntry {
            downloaded: 0,
            total: None,
            last_update: Instant::now(),
            last_pct_byte: 0,
        });

    if !force && !should_update(entry, force, pct_byte) {
        return;
    }

    entry.downloaded = downloaded;
    entry.total = total;
    entry.last_update = Instant::now();
    entry.last_pct_byte = pct_byte;

    if state.is_tty() {
        render_tty(&mut state);
    } else {
        log_throttled(&mut state, filename, downloaded, total);
    }
}

fn finish_progress(filename: &str) {
    let mut state = PROGRESS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.active.remove(filename);

    if !state.is_tty() {
        return;
    }

    if state.active.is_empty() {
        state.rendered_lines = 0;
        let _ = std::io::stderr().lock().flush();
        return;
    }

    render_tty(&mut state);
}

struct ProgressSession {
    filename: String,
}

impl ProgressSession {
    fn begin(filename: &str, downloaded: u64, total: Option<u64>) -> Self {
        report_progress(filename, downloaded, total, true);
        Self {
            filename: filename.to_string(),
        }
    }

    fn update(&self, downloaded: u64, total: Option<u64>, force: bool) {
        report_progress(&self.filename, downloaded, total, force);
    }
}

impl Drop for ProgressSession {
    fn drop(&mut self) {
        finish_progress(&self.filename);
    }
}

/// Removes `path` on drop unless [`PartCleanup::disarm`] is called.
struct PartCleanup {
    path: Option<PathBuf>,
}

impl PartCleanup {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for PartCleanup {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Reject non-HTTPS URLs (docs require HTTPS; blocks `file://` / cleartext MITM).
pub(crate) fn validate_https_url(url: &str) -> Result<(), LlmProviderError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(LlmProviderError::Provider(
            "local model url is empty".to_string(),
        ));
    }
    let Some(rest) = trimmed.strip_prefix("https://") else {
        return Err(LlmProviderError::Provider(format!(
            "local model url must be https:// (got {trimmed:?})"
        )));
    };
    if rest.is_empty() || !rest.contains('.') {
        return Err(LlmProviderError::Provider(format!(
            "local model url is not a valid https URL: {trimmed:?}"
        )));
    }
    Ok(())
}

/// True when `path` exists and starts with the GGUF magic bytes.
pub async fn file_has_gguf_magic(path: &Path) -> bool {
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return false;
    };
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).await.is_ok() && magic == *GGUF_MAGIC
}

fn strip_url_path(url: &str) -> &str {
    let without_scheme = url
        .trim()
        .strip_prefix("https://")
        .or_else(|| url.trim().strip_prefix("http://"))
        .unwrap_or(url.trim());
    let path = without_scheme
        .split_once('/')
        .map_or(without_scheme, |(_, path)| path);
    let path = path.split_once('?').map_or(path, |(p, _)| p);
    path.split_once('#').map_or(path, |(p, _)| p)
}

fn sanitize_basename(segment: &str) -> Result<String, LlmProviderError> {
    if segment.is_empty()
        || segment.contains("..")
        || segment.contains('/')
        || segment.contains('\\')
        || segment.contains('\0')
    {
        return Err(LlmProviderError::Provider(format!(
            "GGUF URL path segment is unsafe: {segment:?}"
        )));
    }
    let stem = Path::new(segment)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            LlmProviderError::Provider(format!(
                "cannot derive GGUF basename from segment: {segment:?}"
            ))
        })?;
    let safe: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        return Err(LlmProviderError::Provider(format!(
            "GGUF basename sanitized to empty from {segment:?}"
        )));
    }
    Ok(safe)
}

/// Derive a stable cache filename from an HTTPS URL (`{stem}-{hash12}.gguf`).
pub fn filename_from_url(url: &str) -> Result<String, LlmProviderError> {
    validate_https_url(url)?;
    let path = strip_url_path(url);
    let segment = path
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty() && s.contains('.'))
        .ok_or_else(|| {
            LlmProviderError::Provider(format!("cannot derive GGUF filename from URL: {url}"))
        })?;
    let stem = sanitize_basename(segment)?;
    let hash = blake3::hash(url.trim().as_bytes());
    let hex = hash.to_hex();
    // blake3 hex is ASCII; take a fixed prefix without UTF-8 slicing risk.
    let short: String = hex.chars().take(CACHE_HASH_HEX_LEN).collect();
    Ok(format!("{stem}-{short}.gguf"))
}

/// Download `url` to `dest`, skipping when a valid GGUF already exists.
pub async fn download_gguf(url: &str, dest: &Path) -> Result<(), LlmProviderError> {
    validate_https_url(url)?;

    if dest.is_file() {
        if file_has_gguf_magic(dest).await {
            tracing::debug!(
                component = "GgufDownload",
                path = %dest.display(),
                "GGUF already present; skipping download"
            );
            return Ok(());
        }
        tracing::warn!(
            component = "GgufDownload",
            path = %dest.display(),
            "cached file lacks GGUF magic; re-downloading"
        );
        let _ = tokio::fs::remove_file(dest).await;
    }

    let key = dest.to_string_lossy().into_owned();
    let slot = {
        let mut guard = IN_FLIGHT.lock().await;
        if let Some(existing) = guard.get(&key) {
            existing.clone()
        } else {
            let slot = std::sync::Arc::new(AsyncMutex::new(None));
            guard.insert(key.clone(), slot.clone());
            slot
        }
    };

    let mut guard = slot.lock().await;
    if let Some(result) = guard.as_ref() {
        return match result {
            Ok(()) => Ok(()),
            Err(e) => Err(LlmProviderError::Provider(e.clone())),
        };
    }

    let result = download_gguf_inner(url, dest)
        .await
        .map_err(|e| e.to_string());
    *guard = Some(result.clone());
    drop(guard);

    let mut in_flight = IN_FLIGHT.lock().await;
    in_flight.remove(&key);

    result.map_err(LlmProviderError::Provider)
}

async fn download_gguf_inner(url: &str, dest: &Path) -> Result<(), LlmProviderError> {
    validate_https_url(url)?;

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            LlmProviderError::Provider(format!(
                "failed to create GGUF cache directory {}: {e}",
                parent.display()
            ))
        })?;
    }

    let filename = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("model.gguf");

    let response =
        http_client()?.get(url.trim()).send().await.map_err(|e| {
            LlmProviderError::Provider(format!("GGUF download request failed: {e}"))
        })?;

    if response.status().is_redirection() {
        return Err(LlmProviderError::Provider(format!(
            "GGUF download refused HTTP redirect ({}): redirects are disabled",
            response.status()
        )));
    }

    if !response.status().is_success() {
        return Err(LlmProviderError::Provider(format!(
            "GGUF download failed with HTTP {}",
            response.status()
        )));
    }

    let total = response.content_length().ok_or_else(|| {
        LlmProviderError::Provider(
            "GGUF download response missing Content-Length; refusing incomplete transfer"
                .to_string(),
        )
    })?;
    if total == 0 {
        return Err(LlmProviderError::Provider(
            "GGUF download Content-Length is zero".to_string(),
        ));
    }
    if total > MAX_DOWNLOAD_BYTES {
        return Err(LlmProviderError::Provider(format!(
            "GGUF download Content-Length {total} exceeds max {MAX_DOWNLOAD_BYTES}"
        )));
    }

    let part_path = dest.with_extension("part");
    let mut cleanup = PartCleanup::new(part_path.clone());
    let mut file = tokio::fs::File::create(&part_path).await.map_err(|e| {
        LlmProviderError::Provider(format!(
            "failed to create GGUF part file {}: {e}",
            part_path.display()
        ))
    })?;

    let progress = ProgressSession::begin(filename, 0, Some(total));

    let mut downloaded: u64 = 0;
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| LlmProviderError::Provider(format!("GGUF download stream error: {e}")))?
    {
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded > total {
            return Err(LlmProviderError::Provider(format!(
                "GGUF download exceeded Content-Length ({downloaded} > {total})"
            )));
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| LlmProviderError::Provider(format!("failed to write GGUF chunk: {e}")))?;
        progress.update(downloaded, Some(total), false);
    }

    if downloaded != total {
        return Err(LlmProviderError::Provider(format!(
            "GGUF download incomplete: got {downloaded} bytes, expected Content-Length {total}"
        )));
    }

    file.flush()
        .await
        .map_err(|e| LlmProviderError::Provider(format!("failed to flush GGUF part file: {e}")))?;
    drop(file);

    if !file_has_gguf_magic(&part_path).await {
        return Err(LlmProviderError::Provider(format!(
            "downloaded file is not a GGUF (missing magic) at {}",
            part_path.display()
        )));
    }

    tokio::fs::rename(&part_path, dest).await.map_err(|e| {
        LlmProviderError::Provider(format!(
            "failed to finalize GGUF download {}: {e}",
            dest.display()
        ))
    })?;
    cleanup.disarm();

    progress.update(downloaded, Some(total), true);
    tracing::info!(
        component = "GgufDownload",
        path = %dest.display(),
        bytes = downloaded,
        "GGUF download complete"
    );
    Ok(())
}

/// Directory where downloaded GGUF weights are stored.
pub fn gguf_cache_dir() -> PathBuf {
    ene_config::models_dir().join("gguf")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn validate_https_rejects_http_and_file() {
        assert!(validate_https_url("https://cdn.example/model.gguf").is_ok());
        assert!(validate_https_url("http://cdn.example/model.gguf").is_err());
        assert!(validate_https_url("file:///tmp/model.gguf").is_err());
        assert!(validate_https_url("").is_err());
    }

    #[test]
    fn filename_rejects_traversal_and_separators() {
        assert!(filename_from_url("https://cdn.example/foo/..evil.gguf").is_err());
        assert!(filename_from_url("https://cdn.example/foo/..%2Fevil.gguf").is_err());
        assert!(filename_from_url("https://cdn.example/foo/bar\\baz.gguf").is_err());
        let ok = filename_from_url("https://cdn.example/models/safe.gguf").expect("safe");
        assert!(!ok.contains(".."));
        assert!(!ok.contains('/'));
        assert!(!ok.contains('\\'));
    }

    #[test]
    fn filename_strips_query_and_is_stable_per_url() {
        let a = filename_from_url("https://cdn.example/models/v5-small.gguf?download=true")
            .expect("url");
        let b = filename_from_url("https://cdn.example/models/v5-small.gguf?download=true")
            .expect("url");
        assert_eq!(a, b);
        assert!(a.starts_with("v5-small-"));
        assert!(a.ends_with(".gguf"));
        assert!(!a.contains('?'));
    }

    #[test]
    fn filename_differs_for_same_basename_different_urls() {
        let a = filename_from_url("https://cdn.example/repo-a/model.gguf").expect("a");
        let b = filename_from_url("https://cdn.example/repo-b/model.gguf").expect("b");
        assert_ne!(a, b);
        assert!(a.starts_with("model-"));
        assert!(b.starts_with("model-"));
    }

    #[tokio::test]
    async fn file_has_gguf_magic_detects_header() {
        let dir = tempdir().expect("tempdir");
        let good = dir.path().join("good.gguf");
        let bad = dir.path().join("bad.gguf");
        {
            let mut f = std::fs::File::create(&good).expect("create");
            f.write_all(b"GGUF\x00\x00\x00\x01").expect("write");
        }
        {
            let mut f = std::fs::File::create(&bad).expect("create");
            f.write_all(b"NOTG").expect("write");
        }
        assert!(file_has_gguf_magic(&good).await);
        assert!(!file_has_gguf_magic(&bad).await);
    }

    #[tokio::test]
    async fn part_cleanup_removes_file_on_drop() {
        let dir = tempdir().expect("tempdir");
        let part = dir.path().join("x.part");
        std::fs::write(&part, b"partial").expect("write");
        {
            let _cleanup = PartCleanup::new(part.clone());
        }
        assert!(!part.exists());
    }

    #[tokio::test]
    async fn part_cleanup_disarm_keeps_file() {
        let dir = tempdir().expect("tempdir");
        let part = dir.path().join("x.part");
        std::fs::write(&part, b"ok").expect("write");
        {
            let mut cleanup = PartCleanup::new(part.clone());
            cleanup.disarm();
        }
        assert!(part.exists());
    }
}
