pub(crate) mod backup;
mod client_id;
mod error;
mod exclusive;
mod lanes;
mod routes;
mod ws;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use axum::Router;
use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{delete, get, patch, post};
use ene_api::SendMessageResponse;
use ene_kernel::{ConversationModel, EchoModel};
use ene_plane::PendingPopup;
use parking_lot::Mutex;
use serde_json::json;
use tokio::sync::oneshot;

use crate::{CoreDaemon, CoreError};
use error::{ApiReject, unauthorized};
use exclusive::ExclusiveHub;
use lanes::LaneHub;
use ws::CoreBus;

/// Shared HTTP state.
#[derive(Clone)]
pub struct AppState {
    pub core: Arc<CoreDaemon>,
    pub lanes: Arc<LaneHub>,
    pub exclusive: Arc<ExclusiveHub>,
    pub idem: Arc<Mutex<HashMap<String, SendMessageResponse>>>,
    pub token: String,
    pub popup: Arc<PendingPopup>,
    pub events: CoreBus,
    pub bind: SocketAddr,
}

/// Running HTTP/WS server.
pub struct ServerHandle {
    /// Bound listen address (port may have been ephemeral).
    pub addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<()>>,
}

impl ServerHandle {
    /// Ask the server to stop and wait for the accept loop to exit.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take()
            && tx.send(()).is_err()
        {
            // Server task already exited.
        }
        if let Some(join) = self.join.take() {
            drop(join.await);
        }
    }
}

impl CoreDaemon {
    /// Bind HTTP/WS with the echo model (offline / tests).
    pub async fn serve(self: Arc<Self>) -> Result<ServerHandle, CoreError> {
        self.serve_with(Arc::new(EchoModel) as Arc<dyn ConversationModel>)
            .await
    }

    /// Bind HTTP/WS using `core.server.bind` (port 0 is ephemeral).
    pub async fn serve_with(
        self: Arc<Self>,
        model: Arc<dyn ConversationModel>,
    ) -> Result<ServerHandle, CoreError> {
        let bind: SocketAddr = self
            .settings()
            .server
            .bind
            .parse()
            .map_err(|err| CoreError::Http(format!("bad bind: {err}")))?;
        self.serve_at(bind, model).await
    }

    /// Bind a specific address (tests).
    pub async fn serve_at(
        self: Arc<Self>,
        bind: SocketAddr,
        model: Arc<dyn ConversationModel>,
    ) -> Result<ServerHandle, CoreError> {
        let listener = tokio::net::TcpListener::bind(bind)
            .await
            .map_err(|err| CoreError::Http(err.to_string()))?;
        let addr = listener
            .local_addr()
            .map_err(|err| CoreError::Http(err.to_string()))?;
        let token = load_or_create_token(self.data_dir(), &self.settings().server.token_file)?;
        write_ready_file(self.data_dir(), addr, &self.settings().server.token_file)?;
        let last_used = self.settings().clients.audio_active_policy == "last_used";
        let state = AppState {
            popup: Arc::clone(self.popup()),
            core: Arc::clone(&self),
            lanes: Arc::new(LaneHub::new(model)),
            exclusive: Arc::new(ExclusiveHub::new(last_used)),
            idem: Arc::new(Mutex::new(HashMap::new())),
            token: token.clone(),
            events: CoreBus::new(self.settings().server.ws_send_buffer),
            bind: addr,
        };
        let app = router(state.clone());
        let (tx, rx) = oneshot::channel::<()>();
        let join = tokio::spawn(async move {
            let serve = axum::serve(listener, app).with_graceful_shutdown(async {
                drop(rx.await);
            });
            drop(serve.await);
        });
        tracing::info!(%addr, "http/ws listening");
        Ok(ServerHandle {
            addr,
            shutdown: Some(tx),
            join: Some(join),
        })
    }
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(routes::web_index))
        .route("/web", get(routes::web_index))
        .route("/api/v1/health", get(routes::health))
        .route("/api/v1/openapi.json", get(routes::openapi))
        .route("/api/v1/souls", get(routes::list_souls))
        .route("/api/v1/souls/{id}", get(routes::get_soul))
        .route("/api/v1/souls/{id}/body", patch(routes::patch_soul_body))
        .route("/api/v1/souls/{id}/affect", get(routes::get_soul_affect))
        .route("/api/v1/souls/{id}/memories", get(routes::list_memories))
        .route("/api/v1/stage", get(routes::get_stage))
        .route(
            "/api/v1/sessions",
            get(routes::list_sessions).post(routes::create_session),
        )
        .route(
            "/api/v1/sessions/{id}",
            get(routes::get_session).patch(routes::patch_session),
        )
        .route("/api/v1/sessions/{id}/fork", post(routes::fork_session))
        .route("/api/v1/sessions/{id}/split", post(routes::split_session))
        .route("/api/v1/sessions/{id}/end", post(routes::end_session))
        .route("/api/v1/sessions/{id}/barge-in", post(routes::barge_in))
        .route("/api/v1/sessions/{id}/export", post(routes::export_session))
        .route("/api/v1/sessions/{id}/messages", post(routes::send_message))
        .route("/api/v1/sessions/{id}/history", get(routes::history))
        .route(
            "/api/v1/sessions/{id}/queued/{entry_id}",
            delete(routes::cancel_queued),
        )
        .route("/api/v1/sessions/{id}/compact", post(routes::compact))
        .route("/api/v1/turns/{id}/cancel", post(routes::cancel_turn))
        .route("/api/v1/jobs", get(routes::list_jobs))
        .route("/api/v1/jobs/{id}", get(routes::get_job))
        .route("/api/v1/jobs/{id}/cancel", post(routes::cancel_job))
        .route(
            "/api/v1/schedules",
            get(routes::list_schedules).post(routes::create_schedule),
        )
        .route(
            "/api/v1/schedules/{id}",
            patch(routes::patch_schedule).delete(routes::delete_schedule),
        )
        .route("/api/v1/artifacts", get(routes::list_artifacts))
        .route(
            "/api/v1/artifacts/{id}/content",
            get(routes::artifact_content),
        )
        .route(
            "/api/v1/memories/{id}",
            patch(routes::patch_memory).delete(routes::delete_memory),
        )
        .route(
            "/api/v1/memories/pending",
            get(routes::list_pending_memories),
        )
        .route(
            "/api/v1/memories/candidates/{id}/resolve",
            post(routes::resolve_memory_candidate),
        )
        .route("/api/v1/tools", get(routes::list_tools))
        .route("/api/v1/tools/{name}/test", post(routes::test_tool))
        .route("/api/v1/plugins", get(routes::list_plugins))
        .route("/api/v1/plugins/{id}/restart", post(routes::restart_plugin))
        .route("/api/v1/approvals", get(routes::list_approvals))
        .route(
            "/api/v1/approvals/{id}/respond",
            post(routes::respond_approval),
        )
        .route("/api/v1/characters", get(routes::list_characters))
        .route("/api/v1/characters/import", post(routes::import_character))
        .route(
            "/api/v1/characters/{id}/export",
            get(routes::export_character),
        )
        .route(
            "/api/v1/settings",
            get(routes::get_settings).patch(routes::patch_settings),
        )
        .route("/api/v1/settings/schema", get(routes::settings_schema))
        .route("/api/v1/audit", get(routes::audit))
        .route("/api/v1/usage", get(routes::usage))
        .route("/api/v1/diag/spans", get(routes::diag_spans))
        .route("/api/v1/backup", post(routes::backup))
        .route("/api/v1/restore", post(routes::restore))
        .route("/api/v1/exclusive", get(routes::exclusive_get))
        .route(
            "/api/v1/exclusive/{resource}",
            post(routes::exclusive_claim).delete(routes::exclusive_release),
        )
        .route("/api/v1/events", get(ws::events))
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state)
}

async fn auth(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiReject> {
    let path = request.uri().path().to_owned();
    if path == "/api/v1/health" || path == "/" || path == "/web" || path.starts_with("/web/") {
        return Ok(next.run(request).await);
    }
    let provided = token_from(&request);
    if !token_matches(provided.as_deref(), state.token.as_str()) {
        drop(
            state
                .core
                .plane()
                .audit()
                .append("auth", &json!({ "ok": false, "path": path })),
        );
        return Err(unauthorized());
    }
    Ok(next.run(request).await)
}

fn token_from(request: &Request) -> Option<String> {
    if let Some(header) = request.headers().get("sec-websocket-protocol")
        && let Ok(value) = header.to_str()
    {
        for proto in value.split(',').map(str::trim) {
            if let Some(token) = proto.strip_prefix("bearer.") {
                return Some(token.to_owned());
            }
        }
    }
    if let Some(header) = request.headers().get(axum::http::header::AUTHORIZATION)
        && let Ok(value) = header.to_str()
        && let Some(token) = value.strip_prefix("Bearer ")
    {
        return Some(token.to_owned());
    }
    if request.uri().path() != "/api/v1/events" {
        return None;
    }
    let query = request.uri().query()?;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next()?;
        let value = parts.next().unwrap_or("");
        if key == "access_token" {
            return Some(url_decode(value));
        }
    }
    None
}

fn token_matches(provided: Option<&str>, expected: &str) -> bool {
    if expected.is_empty() {
        return false;
    }
    let Some(provided) = provided else {
        return false;
    };
    let a = provided.as_bytes();
    let b = expected.as_bytes();
    let mut diff = u8::from(a.len() != b.len());
    for (left, right) in a.iter().chain(b.iter()).zip(b.iter().chain(a.iter())) {
        diff |= left ^ right;
    }
    diff == 0
}

fn url_decode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2]))
        {
            out.push(char::from((hi << 4) | lo));
            i += 3;
            continue;
        }
        if bytes[i] == b'+' {
            out.push(' ');
        } else {
            out.push(char::from(bytes[i]));
        }
        i += 1;
    }
    out
}

const fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn load_or_create_token(data_dir: &Path, token_file: &str) -> Result<String, CoreError> {
    let path = if Path::new(token_file).is_absolute() {
        PathBuf::from(token_file)
    } else {
        data_dir.join(token_file)
    };
    if path.is_file() {
        let raw = std::fs::read_to_string(&path)?;
        let token = raw.trim();
        if !token.is_empty() {
            return Ok(token.to_owned());
        }
    }
    let token =
        uuid::Uuid::new_v4().simple().to_string() + &uuid::Uuid::new_v4().simple().to_string();
    write_token_file(&path, &token)?;
    Ok(token)
}

fn write_token_file(path: &Path, token: &str) -> Result<(), CoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(token.as_bytes())?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, token)?;
        Ok(())
    }
}

fn write_ready_file(data_dir: &Path, addr: SocketAddr, token_file: &str) -> Result<(), CoreError> {
    let body = json!({
        "bind": addr.to_string(),
        "url": format!("http://{addr}"),
        "token_file": token_file,
    });
    std::fs::write(data_dir.join("api.json"), body.to_string())?;
    Ok(())
}
