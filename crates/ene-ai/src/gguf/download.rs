//! Streaming GGUF download with in-place terminal progress (TTY) or throttled logs.

use crate::error::LlmProviderError;
use std::collections::{BTreeMap, HashMap};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex as AsyncMutex;

const PROGRESS_BAR_WIDTH: usize = 10;
const UPDATE_INTERVAL: Duration = Duration::from_millis(500);

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
        *self.tty.get_or_insert_with(|| std::io::stderr().is_terminal())
    }
}

fn http_client() -> Result<&'static reqwest::Client, LlmProviderError> {
    HTTP_CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_hours(1))
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
    let mut state = PROGRESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

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
    let mut state = PROGRESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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

/// Download `url` to `dest`, skipping when the file already exists.
pub async fn download_gguf(url: &str, dest: &Path) -> Result<(), LlmProviderError> {
    if dest.is_file() {
        tracing::debug!(
            component = "GgufDownload",
            path = %dest.display(),
            "GGUF already present; skipping download"
        );
        return Ok(());
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
    if url.trim().is_empty() {
        return Err(LlmProviderError::Provider(
            "local model url is empty".to_string(),
        ));
    }

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

    let response = http_client()?
        .get(url)
        .send()
        .await
        .map_err(|e| LlmProviderError::Provider(format!("GGUF download request failed: {e}")))?;

    if !response.status().is_success() {
        return Err(LlmProviderError::Provider(format!(
            "GGUF download failed with HTTP {}",
            response.status()
        )));
    }

    let total = response.content_length();
    let part_path = dest.with_extension("part");
    let mut file = tokio::fs::File::create(&part_path).await.map_err(|e| {
        LlmProviderError::Provider(format!(
            "failed to create GGUF part file {}: {e}",
            part_path.display()
        ))
    })?;

    let progress = ProgressSession::begin(filename, 0, total);

    let mut downloaded: u64 = 0;
    let mut response = response;
    while let Some(chunk) = response.chunk().await.map_err(|e| {
        LlmProviderError::Provider(format!("GGUF download stream error: {e}"))
    })? {
        file.write_all(&chunk).await.map_err(|e| {
            LlmProviderError::Provider(format!("failed to write GGUF chunk: {e}"))
        })?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        progress.update(downloaded, total, false);
    }

    file.flush().await.map_err(|e| {
        LlmProviderError::Provider(format!("failed to flush GGUF part file: {e}"))
    })?;
    drop(file);

    tokio::fs::rename(&part_path, dest).await.map_err(|e| {
        LlmProviderError::Provider(format!(
            "failed to finalize GGUF download {}: {e}",
            dest.display()
        ))
    })?;

    progress.update(downloaded, total.or(Some(downloaded)), true);
    tracing::info!(
        component = "GgufDownload",
        path = %dest.display(),
        bytes = downloaded,
        "GGUF download complete"
    );
    Ok(())
}

/// Derive a stable cache filename from an HTTPS URL.
pub fn filename_from_url(url: &str) -> Result<String, LlmProviderError> {
    let segment = url
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty() && s.contains('.'))
        .ok_or_else(|| {
            LlmProviderError::Provider(format!(
                "cannot derive GGUF filename from URL: {url}"
            ))
        })?;
    Ok(segment.to_string())
}

/// Directory where downloaded GGUF weights are stored.
pub fn gguf_cache_dir() -> PathBuf {
    ene_config::models_dir().join("gguf")
}
