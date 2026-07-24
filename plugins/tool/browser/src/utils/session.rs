use dashmap::DashMap;
use ene_plugin_proto::ToolError;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_stream::StreamExt;

pub struct BrowserSession {
    pub browser: chromiumoxide::browser::Browser,
    pub page: chromiumoxide::page::Page,
    pub handler_task: tokio::task::JoinHandle<()>,
    pub user_data_dir: std::path::PathBuf,
}

#[derive(Default)]
pub struct BrowserSessionStore {
    sessions: DashMap<String, Arc<Mutex<BrowserSession>>>,
    /// Per-`session_id` async mutex used to serialize the
    /// slow path of `get_or_create` (Chrome launch). The
    /// first caller for a fresh `session_id` acquires the
    /// lock and launches; concurrent callers block on it
    /// and then read the installed session from the map
    /// after releasing. The mutex is never removed: the
    /// per-entry lock has the same lifetime as the
    /// session, and the cost of an unused Mutex after
    /// `close` is negligible.
    creation_locks: DashMap<String, Arc<Mutex<()>>>,
}

impl BrowserSessionStore {
    pub fn new() -> Self {
        // Sweep any `ene-browser-*` user-data-dirs left
        // behind by a previous process that crashed
        // before its `close()` ran. These are full
        // Chromium profile directories (tens to hundreds
        // of MB) and accumulate quickly across crashes.
        // The sweep is best-effort: failures are logged
        // and the store still starts up.
        Self::sweep_stale_dirs();
        Self {
            sessions: DashMap::new(),
            creation_locks: DashMap::new(),
        }
    }

    /// Removes stale `ene-browser-*` directories from
    /// `std::env::temp_dir()`. The directory layout is
    /// `<temp>/ene-browser-<uuid>` and is created
    /// exclusively by [`get_or_create`](Self::get_or_create),
    /// so the prefix is a safe namespace marker.
    ///
    /// A directory is only considered stale when its
    /// modification time is older than [`STALE_DIR_AGE`].
    /// This prevents corrupting active sessions owned by
    /// concurrently running processes.
    fn sweep_stale_dirs() {
        /// Directories younger than this are assumed to
        /// belong to a live process and are left alone.
        const STALE_DIR_AGE: std::time::Duration = std::time::Duration::from_mins(5);

        let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
            return;
        };
        let now = std::time::SystemTime::now();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            if !name_str.starts_with("ene-browser-") {
                continue;
            }
            let path = entry.path();
            // Only remove if it looks like a directory.
            // A stray file matching the prefix (e.g. a
            // user-created `ene-browser-foo.txt`) must not
            // be deleted.
            if !path.is_dir() {
                continue;
            }
            // Skip directories whose metadata cannot be
            // read or whose mtime is within the grace
            // period — they may belong to a live session
            // in another process.
            let is_stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|mtime| now.duration_since(mtime).ok())
                .is_some_and(|age| age >= STALE_DIR_AGE);
            if !is_stale {
                continue;
            }
            if let Err(e) = std::fs::remove_dir_all(&path) {
                tracing::warn!(
                    "[BrowserSessionStore] Failed to sweep stale user-data-dir {}: {e}",
                    path.display()
                );
            }
        }
    }

    /// Returns the session for `session_id`, launching Chrome and
    /// creating it if it does not yet exist.
    ///
    /// Concurrent callers for the same `session_id` are serialized
    /// through a per-session creation lock so that only one Chrome
    /// process is launched; late arrivals observe the session that
    /// the first caller installed.
    pub async fn get_or_create(
        &self,
        session_id: &str,
        chrome_path: std::path::PathBuf,
    ) -> Result<Arc<Mutex<BrowserSession>>, ToolError> {
        // Fast path: an existing session. Avoids the cost
        // of acquiring the per-session creation lock for
        // the common case of reuse.
        if let Some(entry) = self.sessions.get(session_id) {
            return Ok(entry.clone());
        }

        // Slow path: serialize the Chrome launch for this
        // `session_id`. The first caller creates the
        // per-session lock, inserts it into the map, and
        // launches. Concurrent callers for the same
        // `session_id` block on the same lock, then
        // observe the freshly installed session in the
        // fast-path check above.
        //
        // Without this serialization, two concurrent
        // callers for the same `session_id` would both
        // miss the fast-path check, both launch Chrome,
        // and the second `insert` would silently
        // overwrite the first — leaking the orphaned
        // process and its user-data-dir.
        let lock = self
            .creation_locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        // Re-check under the lock: another caller may
        // have completed the launch while we were
        // waiting.
        if let Some(entry) = self.sessions.get(session_id) {
            return Ok(entry.clone());
        }

        let user_data_dir =
            std::env::temp_dir().join(format!("ene-browser-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&user_data_dir).map_err(|e| {
            ToolError::execution_failed(format!("Failed to create user data dir: {e}"))
        })?;

        let config = chromiumoxide::browser::BrowserConfig::builder()
            .chrome_executable(chrome_path)
            .user_data_dir(&user_data_dir)
            .no_sandbox()
            .build()
            .map_err(|e| {
                ToolError::execution_failed(format!("Failed to build browser config: {e}"))
            })?;

        let (browser, mut handler) = chromiumoxide::browser::Browser::launch(config)
            .await
            .map_err(|e| ToolError::execution_failed(format!("Failed to launch browser: {e}")))?;

        let handler_task = tokio::spawn(async move {
            while let Some(h) = handler.next().await {
                if h.is_err() {
                    break;
                }
            }
        });

        let page = browser.new_page("about:blank").await.map_err(|e| {
            // Clean up the half-launched user-data-dir
            // on the failure path so it does not leak.
            let _ = std::fs::remove_dir_all(&user_data_dir);
            ToolError::execution_failed(format!("Failed to create page: {e}"))
        })?;

        let session = BrowserSession {
            browser,
            page,
            handler_task,
            user_data_dir: user_data_dir.clone(),
        };

        let session_arc = Arc::new(Mutex::new(session));
        self.sessions
            .insert(session_id.to_string(), session_arc.clone());

        Ok(session_arc)
    }

    /// Acquires the default session's page, launching Chrome on first use.
    ///
    /// Resolves the Chrome executable, opens (or reuses) the `"default"`
    /// session, and returns a clone of its [`chromiumoxide::page::Page`].
    /// Shared by every browser action so the launch-and-grab boilerplate
    /// lives in one place.
    pub async fn acquire_page(&self) -> Result<chromiumoxide::page::Page, ToolError> {
        let chrome_path =
            crate::utils::chrome::find_chrome_executable().ok_or_else(|| {
                ToolError::execution_failed("No Chrome/Chromium browser found. Please install Google Chrome or Chromium, or set PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH environment variable.".to_string())
            })?;

        let session = self.get_or_create("default", chrome_path).await?;
        let session_guard = session.lock().await;
        Ok(session_guard.page.clone())
    }

    /// Closes and removes the session identified by `session_id`.
    ///
    /// Awaits browser shutdown and user-data-dir removal inline so
    /// that cleanup completes even if the caller's future is dropped
    /// shortly after (e.g. during process shutdown). Returns `true`
    /// if a session existed and was closed, `false` otherwise.
    pub async fn close(&self, session_id: &str) -> bool {
        let existed = if let Some((_, session_arc)) = self.sessions.remove(session_id) {
            let mut session = session_arc.lock().await;
            session.handler_task.abort();
            let _ = session.browser.close().await;
            let dir = session.user_data_dir.clone();
            drop(session);
            let _ = tokio::fs::remove_dir_all(&dir).await;
            true
        } else {
            false
        };
        // Also drop the per-session creation lock so the
        // map does not grow unboundedly across many
        // `close` cycles. A fresh `get_or_create` for
        // the same `session_id` will lazily re-insert
        // the lock on demand.
        self.creation_locks.remove(session_id);
        existed
    }

    /// Shuts down all active sessions, closing browsers and removing
    /// user-data-dirs. Called once during process teardown.
    pub async fn shutdown(&self) {
        let keys: Vec<String> = self
            .sessions
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        for key in keys {
            if let Some((_, session_arc)) = self.sessions.remove(&key) {
                let mut session = session_arc.lock().await;
                session.handler_task.abort();
                let _ = session.browser.close().await;
                let dir = session.user_data_dir.clone();
                let _ = tokio::fs::remove_dir_all(&dir).await;
            }
        }
        self.creation_locks.clear();
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test fixtures use unwrap for temp dir setup"
)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn close_nonexistent_session_returns_false() {
        let store = BrowserSessionStore {
            sessions: DashMap::new(),
            creation_locks: DashMap::new(),
        };
        assert!(!store.close("default").await);
        assert!(!store.close("nonexistent").await);
    }

    #[tokio::test]
    async fn shutdown_empty_store_does_not_panic() {
        let store = BrowserSessionStore {
            sessions: DashMap::new(),
            creation_locks: DashMap::new(),
        };
        store.shutdown().await;
        assert!(store.sessions.is_empty());
    }

    #[test]
    fn sweep_stale_dirs_skips_fresh_dirs() {
        let dir = std::env::temp_dir().join(format!("ene-browser-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // A freshly created dir has mtime ≈ now, well within the 5-min
        // threshold, so sweep must not delete it.
        BrowserSessionStore::sweep_stale_dirs();
        assert!(dir.exists(), "fresh dir must survive sweep");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sweep_stale_dirs_ignores_non_matching_prefix() {
        let dir = std::env::temp_dir().join(format!("other-prefix-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        BrowserSessionStore::sweep_stale_dirs();
        assert!(dir.exists(), "non-matching prefix must be untouched");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sweep_stale_dirs_ignores_files_with_matching_prefix() {
        let file = std::env::temp_dir().join(format!("ene-browser-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&file, "not a dir").unwrap();

        BrowserSessionStore::sweep_stale_dirs();
        assert!(file.exists(), "file matching prefix must not be deleted");

        std::fs::remove_file(&file).unwrap();
    }
}
