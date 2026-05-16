use crate::error::AiCoreError;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_stream::StreamExt;

pub struct BrowserSession {
    #[allow(dead_code)]
    pub browser: chromiumoxide::browser::Browser,
    pub page: chromiumoxide::page::Page,
    pub handler_task: tokio::task::JoinHandle<()>,
    pub user_data_dir: std::path::PathBuf,
}

#[derive(Default)]
pub struct BrowserSessionStore {
    sessions: DashMap<String, Arc<Mutex<BrowserSession>>>,
}

impl BrowserSessionStore {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
        }
    }

    pub async fn get_or_create(
        &self,
        session_id: &str,
        chrome_path: std::path::PathBuf,
    ) -> Result<Arc<Mutex<BrowserSession>>, AiCoreError> {
        if let Some(entry) = self.sessions.get(session_id) {
            return Ok(entry.clone());
        }

        let user_data_dir =
            std::env::temp_dir().join(format!("ene-browser-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&user_data_dir).map_err(|e| {
            AiCoreError::BrowserError(format!("Failed to create user data dir: {e}"))
        })?;

        let config = chromiumoxide::browser::BrowserConfig::builder()
            .chrome_executable(chrome_path)
            .user_data_dir(&user_data_dir)
            .no_sandbox()
            .build()
            .map_err(|e| {
                AiCoreError::BrowserError(format!("Failed to build browser config: {e}"))
            })?;

        let (browser, mut handler) = chromiumoxide::browser::Browser::launch(config)
            .await
            .map_err(|e| AiCoreError::BrowserError(format!("Failed to launch browser: {e}")))?;

        let handler_task = tokio::spawn(async move {
            while let Some(h) = handler.next().await {
                if h.is_err() {
                    break;
                }
            }
        });

        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| AiCoreError::BrowserError(format!("Failed to create page: {e}")))?;

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
    }
}
