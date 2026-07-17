use dashmap::DashMap;
use ene_tool_proto::ToolError;
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

    /// Removes every `ene-browser-*` directory in
    /// `std::env::temp_dir()`. The directory layout is
    /// `<temp>/ene-browser-<uuid>` and is created
    /// exclusively by [`get_or_create`](Self::get_or_create),
    /// so the prefix is a safe namespace marker.
    fn sweep_stale_dirs() {
        let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
            return;
        };
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
            if path.is_dir()
                && let Err(e) = std::fs::remove_dir_all(&path)
            {
                eprintln!(
                    "[BrowserSessionStore] Failed to sweep stale user-data-dir {}: {e}",
                    path.display()
                );
            }
        }
    }

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
        std::fs::create_dir_all(&user_data_dir).map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to create user data dir: {e}"),
        })?;

        let config = chromiumoxide::browser::BrowserConfig::builder()
            .chrome_executable(chrome_path)
            .user_data_dir(&user_data_dir)
            .no_sandbox()
            .build()
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to build browser config: {e}"),
            })?;

        let (browser, mut handler) = chromiumoxide::browser::Browser::launch(config)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to launch browser: {e}"),
            })?;

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
            ToolError::ExecutionFailed {
                message: format!("Failed to create page: {e}"),
            }
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

    pub fn close(&self, session_id: &str) {
        if let Some((_, session_arc)) = self.sessions.remove(session_id) {
            tokio::spawn(async move {
                let session = session_arc.lock().await;
                session.handler_task.abort();
                let dir = session.user_data_dir.clone();
                drop(session);
                let _ = tokio::fs::remove_dir_all(&dir).await;
            });
        }
        // Also drop the per-session creation lock so the
        // map does not grow unboundedly across many
        // `close` cycles. A fresh `get_or_create` for
        // the same `session_id` will lazily re-insert
        // the lock on demand.
        self.creation_locks.remove(session_id);
    }

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
