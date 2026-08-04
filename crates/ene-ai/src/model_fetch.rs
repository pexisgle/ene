//! Shared, safe model-file downloader for local inference backends.
//!
//! The local-llm provider plugin (`plugins/provider/local-llm`,
//! GGUF/llama.cpp) and [`ene-voice`](../../ene_voice/index.html) (Kokoro
//! ONNX TTS) both need to fetch large model weights from the network before
//! a local engine can start. This module is the one place that does it
//! safely, so neither engine has to re-derive the same safety properties:
//!
//! - **In-flight coalescing** — concurrent requests for the same
//!   destination path share one download instead of racing to write the
//!   same file.
//! - **A shared [`reqwest::Client`]** behind a [`OnceLock`], built once per
//!   process.
//! - **`.part` file + atomic rename** — the destination path never observes
//!   a partially-written file.
//! - **RAII cleanup** of the `.part` file on any failure path (including a
//!   panic unwinding through the download), via [`PartCleanup`].
//! - **HTTPS-only enforcement** ([`validate_https_url`]), including across
//!   redirects.
//! - **Pluggable post-download validation** via the [`ModelValidator`]
//!   trait, so format-specific checks (GGUF magic bytes, an ONNX structural
//!   heuristic, a raw `f32` blob's size/alignment) live with their callers
//!   instead of being hardcoded here.
//! - **Progress reporting** to stderr (in-place TTY bar, or throttled log
//!   lines when not a terminal).
//!
//! # Validation is the pluggable part
//!
//! [`ModelValidator`] is deliberately minimal (one method: does this file
//! look valid). [`MagicBytesValidator`], [`SizeMultipleValidator`], and
//! [`PrefixPredicateValidator`] are the generic building blocks this module
//! provides; each caller wires up the format-specific constant:
//!
//! - GGUF has a real 4-byte magic (`b"GGUF"`) → [`MagicBytesValidator`].
//! - ONNX (`ModelProto`) is protobuf-encoded and has no fixed signature at
//!   all → [`PrefixPredicateValidator`] with a protobuf-wire-type sanity
//!   check, which is enough to reject the failure mode this validator
//!   exists for (a truncated download or an HTML/JSON error page landing in
//!   place of the model).
//! - Kokoro's `voices.bin` is a flat, headerless `f32` array →
//!   [`SizeMultipleValidator`], checking the file is a whole number of
//!   embedding-sized elements — the honest equivalent of a signature check
//!   for a format that has none.

use std::collections::{BTreeMap, HashMap};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex as AsyncMutex;

const PROGRESS_BAR_WIDTH: usize = 10;
const UPDATE_INTERVAL: Duration = Duration::from_millis(500);
/// Hard cap on a single model download (30 GiB), shared by every caller of
/// [`ModelFetcher`].
const MAX_DOWNLOAD_BYTES: u64 = 30 * 1024 * 1024 * 1024;
/// Hugging Face `resolve/` URLs 302 to CDN hosts; follow HTTPS→HTTPS only.
const MAX_HTTPS_REDIRECTS: usize = 5;

type InflightSlot = Arc<AsyncMutex<Option<Result<(), String>>>>;

static HTTP_CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
static IN_FLIGHT: LazyLock<AsyncMutex<HashMap<String, InflightSlot>>> =
    LazyLock::new(|| AsyncMutex::new(HashMap::new()));
static PROGRESS: Mutex<ProgressState> = Mutex::new(ProgressState::new());

fn http_client() -> Result<&'static reqwest::Client, ModelFetchError> {
    HTTP_CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_hours(1))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| ModelFetchError::ClientInit(e.clone()))
}

// ---------------------------------------------------------------------
// Progress reporting
// ---------------------------------------------------------------------

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
            "[ModelFetch] {filename} {bar} {pct:>3.0}% {}/{}",
            format_size(downloaded),
            format_size(total_bytes),
        ),
        None => format!(
            "[ModelFetch] {filename} {bar} {pct:>3.0}% {}",
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
        drop(write!(stderr, "\x1b[{}A", state.rendered_lines));
    }

    for i in 0..slot_count {
        drop(write!(stderr, "\x1b[2K\r"));
        if let Some(line) = lines.get(i) {
            drop(writeln!(stderr, "{line}"));
        } else {
            drop(writeln!(stderr));
        }
    }

    state.rendered_lines = lines.len();
    drop(stderr.flush());
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
        drop(std::io::stderr().lock().flush());
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

// ---------------------------------------------------------------------
// `.part` cleanup
// ---------------------------------------------------------------------

/// Removes `path` on drop unless [`PartCleanup::disarm`] is called.
///
/// Guarantees the `.part` file never lingers on any failure exit (an error
/// return, a cancelled future, or a panic unwinding through the download).
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
            drop(std::fs::remove_file(&path));
        }
    }
}

// ---------------------------------------------------------------------
// URL utilities
// ---------------------------------------------------------------------

/// Reject non-HTTPS URLs (docs require HTTPS; blocks `file://` / cleartext
/// MITM). Used both for the initial request and every redirect hop.
pub fn validate_https_url(url: &str) -> Result<(), ModelFetchError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(ModelFetchError::InvalidUrl("url is empty".to_string()));
    }
    let Some(rest) = trimmed.strip_prefix("https://") else {
        return Err(ModelFetchError::InvalidUrl(format!(
            "url must be https:// (got {trimmed:?})"
        )));
    };
    if rest.is_empty() || !rest.contains('.') {
        return Err(ModelFetchError::InvalidUrl(format!(
            "not a valid https URL: {trimmed:?}"
        )));
    }
    Ok(())
}

/// Strip scheme, query string, and fragment from `url`, returning just the
/// path portion (e.g. `models/foo.gguf?download=true` → `models/foo.gguf`).
pub fn strip_url_path(url: &str) -> &str {
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

/// Sanitize a URL path segment into a safe filename stem: rejects traversal
/// (`..`), path separators, and NUL, then maps any other non-alphanumeric
/// character to `_`.
pub fn sanitize_basename(segment: &str) -> Result<String, ModelFetchError> {
    if segment.is_empty()
        || segment.contains("..")
        || segment.contains('/')
        || segment.contains('\\')
        || segment.contains('\0')
    {
        return Err(ModelFetchError::UnsafeSegment(segment.to_string()));
    }
    let stem = Path::new(segment)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ModelFetchError::UnsafeSegment(segment.to_string()))?;
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
        return Err(ModelFetchError::UnsafeSegment(segment.to_string()));
    }
    Ok(safe)
}

// ---------------------------------------------------------------------
// Validators
// ---------------------------------------------------------------------

/// Pluggable post-download (and cache-hit) validity check for a model file.
///
/// [`ModelFetcher`] calls this both to re-validate a pre-existing cache hit
/// (so a corrupted or truncated cached file gets re-downloaded instead of
/// silently reused forever) and, on a fresh download, to validate the
/// `.part` file's contents before the atomic rename commits it as the final
/// destination.
///
/// Implementations should be cheap — a bounded prefix read or a single
/// `metadata()` call — since this runs on every [`ModelFetcher::fetch`]
/// call, not just the first.
#[async_trait::async_trait]
pub trait ModelValidator: Send + Sync {
    /// Format name used in log lines and error messages (e.g. `"gguf"`).
    fn format_name(&self) -> &'static str;

    /// Returns `true` when `path`'s contents pass this format's validity
    /// check. Called with a fully-written file — never mid-download.
    async fn validate(&self, path: &Path) -> bool;
}

/// Validates a file by comparing its leading bytes against a fixed magic
/// sequence (e.g. GGUF's `b"GGUF"`).
#[derive(Debug, Clone, Copy)]
pub struct MagicBytesValidator {
    format_name: &'static str,
    magic: &'static [u8],
}

impl MagicBytesValidator {
    /// Build a validator that checks `path` starts with `magic`.
    #[must_use]
    pub const fn new(format_name: &'static str, magic: &'static [u8]) -> Self {
        Self { format_name, magic }
    }
}

#[async_trait::async_trait]
impl ModelValidator for MagicBytesValidator {
    fn format_name(&self) -> &'static str {
        self.format_name
    }

    async fn validate(&self, path: &Path) -> bool {
        let Ok(mut file) = tokio::fs::File::open(path).await else {
            return false;
        };
        let mut buf = vec![0u8; self.magic.len()];
        file.read_exact(&mut buf).await.is_ok() && buf == self.magic
    }
}

/// Validates a headerless raw binary blob by checking its size is both at
/// least `min_bytes` and an exact multiple of `element_bytes`.
///
/// The honest equivalent of a signature check for formats with no magic
/// bytes at all — e.g. Kokoro's `voices.bin`, a flat little-endian `f32`
/// array of stacked voice embeddings.
#[derive(Debug, Clone, Copy)]
pub struct SizeMultipleValidator {
    format_name: &'static str,
    element_bytes: u64,
    min_bytes: u64,
}

impl SizeMultipleValidator {
    /// Build a validator requiring at least `min_bytes` and a length evenly
    /// divisible by `element_bytes`.
    #[must_use]
    pub const fn new(format_name: &'static str, element_bytes: u64, min_bytes: u64) -> Self {
        Self {
            format_name,
            element_bytes,
            min_bytes,
        }
    }
}

#[async_trait::async_trait]
impl ModelValidator for SizeMultipleValidator {
    fn format_name(&self) -> &'static str {
        self.format_name
    }

    async fn validate(&self, path: &Path) -> bool {
        let Ok(meta) = tokio::fs::metadata(path).await else {
            return false;
        };
        let len = meta.len();
        self.element_bytes > 0 && len >= self.min_bytes && len % self.element_bytes == 0
    }
}

/// Validates a file by applying a predicate function to a bounded prefix of
/// its bytes — for formats with no fixed magic sequence at all (e.g. ONNX's
/// protobuf-encoded `ModelProto`).
#[derive(Clone, Copy)]
pub struct PrefixPredicateValidator {
    format_name: &'static str,
    min_bytes: u64,
    prefix_len: usize,
    predicate: fn(&[u8]) -> bool,
}

impl std::fmt::Debug for PrefixPredicateValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrefixPredicateValidator")
            .field("format_name", &self.format_name)
            .field("min_bytes", &self.min_bytes)
            .field("prefix_len", &self.prefix_len)
            .finish_non_exhaustive()
    }
}

impl PrefixPredicateValidator {
    /// Build a validator requiring at least `min_bytes` and a leading
    /// `prefix_len`-byte window that satisfies `predicate`.
    #[must_use]
    pub const fn new(
        format_name: &'static str,
        min_bytes: u64,
        prefix_len: usize,
        predicate: fn(&[u8]) -> bool,
    ) -> Self {
        Self {
            format_name,
            min_bytes,
            prefix_len,
            predicate,
        }
    }
}

#[async_trait::async_trait]
impl ModelValidator for PrefixPredicateValidator {
    fn format_name(&self) -> &'static str {
        self.format_name
    }

    async fn validate(&self, path: &Path) -> bool {
        let Ok(meta) = tokio::fs::metadata(path).await else {
            return false;
        };
        if meta.len() < self.min_bytes {
            return false;
        }
        let Ok(mut file) = tokio::fs::File::open(path).await else {
            return false;
        };
        let want = self
            .prefix_len
            .min(usize::try_from(meta.len()).unwrap_or(usize::MAX));
        let mut buf = vec![0u8; want];
        if file.read_exact(&mut buf).await.is_err() {
            return false;
        }
        (self.predicate)(&buf)
    }
}

// ---------------------------------------------------------------------
// ModelFetcher
// ---------------------------------------------------------------------

/// Shared, safe model-file downloader.
///
/// Stateless from the caller's perspective: the in-flight coalescing map
/// and the shared [`reqwest::Client`] are process-wide statics, so every
/// [`ModelFetcher`] instance (however many are constructed) coalesces
/// against the same set of in-flight downloads. Safe to construct on every
/// call.
#[derive(Debug, Default, Clone, Copy)]
pub struct ModelFetcher;

impl ModelFetcher {
    /// Construct a fetcher. Holds no per-instance state.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Ensure a file exists at `dest`, downloading it from `url` if missing
    /// or invalid.
    ///
    /// Behavior:
    /// 1. If `dest` already exists and passes `validator`, returns
    ///    immediately (no network I/O).
    /// 2. If `dest` exists but fails `validator`, it is removed and
    ///    re-downloaded (self-healing a corrupted or truncated cache entry).
    /// 3. Concurrent calls for the same `dest` coalesce onto a single
    ///    download; all callers observe the same result.
    /// 4. The download streams to `dest.with_extension("part")`, which is
    ///    validated and then atomically renamed to `dest`. On any failure
    ///    the `.part` file is removed.
    ///
    /// `url` must be `https://`; this is enforced up front and again on
    /// every redirect hop.
    pub async fn fetch(
        &self,
        url: &str,
        dest: &Path,
        validator: &dyn ModelValidator,
    ) -> Result<(), ModelFetchError> {
        validate_https_url(url)?;

        if dest.is_file() {
            if validator.validate(dest).await {
                tracing::debug!(
                    component = "ModelFetch",
                    format = validator.format_name(),
                    path = %dest.display(),
                    "model file already present; skipping download"
                );
                return Ok(());
            }
            tracing::warn!(
                component = "ModelFetch",
                format = validator.format_name(),
                path = %dest.display(),
                "cached file failed validation; re-downloading"
            );
            drop(tokio::fs::remove_file(dest).await);
        }

        let key = dest.to_string_lossy().into_owned();
        let slot = {
            let mut guard = IN_FLIGHT.lock().await;
            if let Some(existing) = guard.get(&key) {
                existing.clone()
            } else {
                let slot: InflightSlot = Arc::new(AsyncMutex::new(None));
                guard.insert(key.clone(), slot.clone());
                slot
            }
        };

        let mut guard = slot.lock().await;
        if let Some(result) = guard.as_ref() {
            return match result {
                Ok(()) => Ok(()),
                Err(e) => Err(ModelFetchError::Failed(e.clone())),
            };
        }

        let result = fetch_inner(url, dest, validator)
            .await
            .map_err(|e| e.to_string());
        *guard = Some(result.clone());
        drop(guard);

        let mut in_flight = IN_FLIGHT.lock().await;
        in_flight.remove(&key);

        result.map_err(ModelFetchError::Failed)
    }
}

async fn fetch_inner(
    url: &str,
    dest: &Path,
    validator: &dyn ModelValidator,
) -> Result<(), ModelFetchError> {
    validate_https_url(url)?;

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            ModelFetchError::Io(format!("create cache directory {}: {e}", parent.display()))
        })?;
    }

    let filename = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("model.bin");

    let response = fetch_https(url).await?;

    if !response.status().is_success() {
        return Err(ModelFetchError::Http(response.status().as_u16()));
    }

    let total = response
        .content_length()
        .ok_or(ModelFetchError::MissingContentLength)?;
    if total == 0 {
        return Err(ModelFetchError::EmptyContentLength);
    }
    if total > MAX_DOWNLOAD_BYTES {
        return Err(ModelFetchError::TooLarge {
            total,
            max: MAX_DOWNLOAD_BYTES,
        });
    }

    let part_path = dest.with_extension("part");
    let mut cleanup = PartCleanup::new(part_path.clone());
    let mut file = tokio::fs::File::create(&part_path).await.map_err(|e| {
        ModelFetchError::Io(format!("create part file {}: {e}", part_path.display()))
    })?;

    let progress = ProgressSession::begin(filename, 0, Some(total));

    let mut downloaded: u64 = 0;
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| ModelFetchError::Network(e.to_string()))?
    {
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded > total {
            return Err(ModelFetchError::OverLength { downloaded, total });
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| ModelFetchError::Io(format!("write chunk: {e}")))?;
        progress.update(downloaded, Some(total), false);
    }

    if downloaded != total {
        return Err(ModelFetchError::Incomplete { downloaded, total });
    }

    file.flush()
        .await
        .map_err(|e| ModelFetchError::Io(format!("flush part file: {e}")))?;
    drop(file);

    if !validator.validate(&part_path).await {
        return Err(ModelFetchError::Validation {
            format: validator.format_name(),
            path: part_path,
        });
    }

    tokio::fs::rename(&part_path, dest).await.map_err(|e| {
        ModelFetchError::Io(format!("finalize download to {}: {e}", dest.display()))
    })?;
    cleanup.disarm();

    progress.update(downloaded, Some(total), true);
    tracing::info!(
        component = "ModelFetch",
        format = validator.format_name(),
        path = %dest.display(),
        bytes = downloaded,
        "model download complete"
    );
    Ok(())
}

/// GET `url`, following a bounded number of HTTPS→HTTPS redirects (Hugging
/// Face CDN redirects a `resolve/` URL to a signed CDN URL this way).
async fn fetch_https(url: &str) -> Result<reqwest::Response, ModelFetchError> {
    let client = http_client()?;
    let mut current = url.trim().to_string();

    for hop in 0..=MAX_HTTPS_REDIRECTS {
        validate_https_url(&current)?;
        let response = client
            .get(&current)
            .send()
            .await
            .map_err(|e| ModelFetchError::Network(e.to_string()))?;

        if !response.status().is_redirection() {
            return Ok(response);
        }

        if hop == MAX_HTTPS_REDIRECTS {
            return Err(ModelFetchError::TooManyRedirects(MAX_HTTPS_REDIRECTS));
        }

        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                ModelFetchError::Redirect(format!(
                    "redirect ({}) missing Location header",
                    response.status()
                ))
            })?;
        let next = resolve_https_redirect(&current, location)?;
        tracing::debug!(
            component = "ModelFetch",
            from = %current,
            to = %next,
            status = %response.status(),
            hop,
            "following HTTPS redirect"
        );
        current = next;
    }

    Err(ModelFetchError::Redirect("redirect loop".to_string()))
}

fn resolve_https_redirect(base: &str, location: &str) -> Result<String, ModelFetchError> {
    let base_url = reqwest::Url::parse(base.trim())
        .map_err(|e| ModelFetchError::Redirect(format!("invalid redirect base URL: {e}")))?;
    let joined = base_url
        .join(location.trim())
        .map_err(|e| ModelFetchError::Redirect(format!("invalid redirect Location: {e}")))?;
    if joined.scheme() != "https" {
        return Err(ModelFetchError::Redirect(format!(
            "refused non-HTTPS redirect to {}",
            joined.scheme()
        )));
    }
    validate_https_url(joined.as_str())?;
    Ok(joined.to_string())
}

// ---------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------

/// Errors from [`ModelFetcher::fetch`] and its supporting URL utilities.
///
/// A dedicated error type at the `ene-ai` boundary rather than reusing
/// either [`crate::error::LlmProviderError`] or
/// [`crate::traits::AudioProviderError`] directly — those two describe
/// LLM-provider and audio-provider failures respectively, and neither
/// should leak into the other's crate. Both convert from
/// [`ModelFetchError`] via `From`.
#[derive(Debug, thiserror::Error, Clone)]
pub enum ModelFetchError {
    /// The URL is empty, not `https://`, or otherwise malformed.
    #[error("invalid model URL: {0}")]
    InvalidUrl(String),
    /// A filesystem operation (create dir, create/write/flush/rename a
    /// file) failed.
    #[error("model fetch I/O error: {0}")]
    Io(String),
    /// The HTTP request itself failed (connect, TLS, DNS) or the response
    /// body stream errored mid-transfer.
    #[error("model fetch network error: {0}")]
    Network(String),
    /// The shared HTTP client failed to initialize.
    #[error("model fetch HTTP client init failed: {0}")]
    ClientInit(String),
    /// The server responded with a non-success, non-redirect HTTP status.
    #[error("model fetch failed with HTTP {0}")]
    Http(u16),
    /// The response had no `Content-Length`; refused rather than accept an
    /// unbounded, unverifiable transfer.
    #[error("model fetch response missing Content-Length; refusing incomplete transfer")]
    MissingContentLength,
    /// The response declared a zero-byte body.
    #[error("model fetch Content-Length is zero")]
    EmptyContentLength,
    /// The declared `Content-Length` exceeds the shared 30 GiB download cap.
    #[error("model fetch Content-Length {total} exceeds max {max} bytes")]
    TooLarge {
        /// Declared content length, in bytes.
        total: u64,
        /// The configured maximum, in bytes.
        max: u64,
    },
    /// The server sent more bytes than it declared in `Content-Length`.
    #[error("model fetch exceeded Content-Length ({downloaded} > {total})")]
    OverLength {
        /// Bytes received so far.
        downloaded: u64,
        /// Declared content length.
        total: u64,
    },
    /// The stream ended before `Content-Length` bytes were received.
    #[error("model fetch incomplete: got {downloaded} bytes, expected {total}")]
    Incomplete {
        /// Bytes actually received.
        downloaded: u64,
        /// Declared content length.
        total: u64,
    },
    /// The downloaded (or cached) file failed the caller-supplied
    /// [`ModelValidator`].
    #[error("downloaded file failed {format} validation at {path}")]
    Validation {
        /// The validator's [`ModelValidator::format_name`].
        format: &'static str,
        /// The file that failed validation.
        path: PathBuf,
    },
    /// A redirect could not be followed (missing/invalid `Location`, too
    /// many hops, or a non-HTTPS target).
    #[error("model fetch redirect error: {0}")]
    Redirect(String),
    /// Too many HTTPS redirect hops were followed without reaching a
    /// non-redirect response.
    #[error("model fetch exceeded {0} HTTPS redirects")]
    TooManyRedirects(usize),
    /// A URL path segment could not be turned into a safe filename
    /// component (traversal, separators, or NUL).
    #[error("model URL path segment is unsafe: {0:?}")]
    UnsafeSegment(String),
    /// The download attempt failed. Used both for the call that actually
    /// ran the download and for concurrent callers that coalesced onto it —
    /// the underlying typed error does not survive coalescing since the
    /// in-flight slot shares its result across tasks as a `String`.
    #[error("{0}")]
    Failed(String),
}

impl From<ModelFetchError> for crate::error::LlmProviderError {
    fn from(e: ModelFetchError) -> Self {
        Self::Provider(e.to_string())
    }
}

impl From<ModelFetchError> for crate::traits::AudioProviderError {
    fn from(e: ModelFetchError) -> Self {
        Self::Init(e.to_string())
    }
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
    fn resolve_https_redirect_allows_cdn_and_rejects_http() {
        let next = resolve_https_redirect(
            "https://huggingface.co/org/model/resolve/main/model.onnx",
            "https://cdn-lfs.huggingface.co/path/model.onnx",
        )
        .expect("https redirect");
        assert!(next.starts_with("https://cdn-lfs.huggingface.co/"));

        let relative = resolve_https_redirect(
            "https://huggingface.co/org/model/resolve/main/model.onnx",
            "/cdn/model.onnx",
        )
        .expect("relative https");
        assert_eq!(relative, "https://huggingface.co/cdn/model.onnx");

        assert!(
            resolve_https_redirect(
                "https://huggingface.co/org/model/resolve/main/model.onnx",
                "http://evil.example/model.onnx",
            )
            .is_err()
        );
    }

    #[test]
    fn sanitize_basename_rejects_traversal_and_separators() {
        assert!(sanitize_basename("..evil").is_err());
        assert!(sanitize_basename("foo/bar").is_err());
        assert!(sanitize_basename("foo\\bar").is_err());
        assert!(sanitize_basename("").is_err());
        // `file_stem` drops a trailing extension (by design: callers append
        // their own format-specific extension to the sanitized stem).
        let ok = sanitize_basename("safe-name.v1.gguf").expect("safe");
        assert_eq!(ok, "safe-name.v1");
    }

    #[tokio::test]
    async fn magic_bytes_validator_detects_header() {
        let dir = tempdir().expect("tempdir");
        let good = dir.path().join("good.gguf");
        let bad = dir.path().join("bad.gguf");
        std::fs::write(&good, b"GGUF\x00\x00\x00\x01").expect("write good");
        std::fs::write(&bad, b"NOTG").expect("write bad");

        let validator = MagicBytesValidator::new("gguf", b"GGUF");
        assert!(validator.validate(&good).await);
        assert!(!validator.validate(&bad).await);
    }

    #[tokio::test]
    async fn size_multiple_validator_checks_alignment_and_minimum() {
        let dir = tempdir().expect("tempdir");
        let ok_path = dir.path().join("voices-ok.bin");
        let short_path = dir.path().join("voices-short.bin");
        let misaligned_path = dir.path().join("voices-misaligned.bin");
        std::fs::write(&ok_path, vec![0u8; 16]).expect("write ok");
        std::fs::write(&short_path, vec![0u8; 4]).expect("write short");
        std::fs::write(&misaligned_path, vec![0u8; 15]).expect("write misaligned");

        let validator = SizeMultipleValidator::new("voices-bin", 8, 16);
        assert!(validator.validate(&ok_path).await);
        assert!(!validator.validate(&short_path).await);
        assert!(!validator.validate(&misaligned_path).await);
    }

    #[tokio::test]
    async fn prefix_predicate_validator_applies_predicate_to_prefix() {
        let dir = tempdir().expect("tempdir");
        let good = dir.path().join("model.onnx");
        let bad = dir.path().join("error.html");
        std::fs::write(&good, [0x08_u8, 0x01, 0x12, 0x02]).expect("write good");
        std::fs::write(&bad, b"<!DOCTYPE html><html>").expect("write bad");

        fn starts_with_valid_varint_tag(buf: &[u8]) -> bool {
            buf.first()
                .is_some_and(|b| matches!(b & 0x07, 0 | 1 | 2 | 5))
        }

        let validator = PrefixPredicateValidator::new("onnx", 4, 4, starts_with_valid_varint_tag);
        assert!(validator.validate(&good).await);
        assert!(!validator.validate(&bad).await);
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

    #[tokio::test]
    async fn fetch_skips_when_cached_file_already_valid() {
        let dir = tempdir().expect("tempdir");
        let dest = dir.path().join("model.gguf");
        std::fs::write(&dest, b"GGUF\x00\x00\x00\x01").expect("write cached");

        let validator = MagicBytesValidator::new("gguf", b"GGUF");
        let fetcher = ModelFetcher::new();
        // A bogus URL would fail if the fetcher actually tried the network;
        // the cache hit must short-circuit before any request is made.
        let result = fetcher
            .fetch("https://example.invalid/model.gguf", &dest, &validator)
            .await;
        assert!(result.is_ok());
    }
}
