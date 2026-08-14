//! Broker-channel wire types (protocol v8).
//!
//! Plugins never touch the OS directly: user files, downloads, child
//! processes, credentials, and executable artifacts are all mediated by the
//! host through one authenticated broker channel (the host-service socket).
//! Each [`BrokerRequest`] is dispatched to the passenger the plugin opened
//! with [`HostServiceRequest::Open`](crate::HostServiceRequest::Open), and
//! the host's identity binding (per-plugin token) prevents one plugin from
//! impersonating another.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// HTTP method for broker-mediated fetches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HttpMethod {
    /// GET.
    Get,
    /// POST.
    Post,
    /// PUT.
    Put,
    /// DELETE.
    Delete,
    /// HEAD.
    Head,
}

/// How a web-file save resolves a name conflict at the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConflictMode {
    /// Keep the existing file; append a numeric suffix to the new name.
    Rename,
    /// Fail the save when the destination exists (no automatic overwrite).
    Fail,
}

/// Artifact kind on the wire (mirrors `ene-artifact::catalog::ArtifactKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WireArtifactKind {
    /// Plugin binary.
    Plugin,
    /// Sidecar binary.
    Sidecar,
    /// Model file.
    Model,
}

/// One directory entry from [`BrokerResponse::FileListOk`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileEntry {
    /// Entry name (basename).
    pub name: String,
    /// Logical path as requested.
    pub path: String,
    /// Whether this is a directory.
    pub is_dir: bool,
    /// File size in bytes (0 for directories).
    pub size: u64,
    /// Last-modified Unix milliseconds (0 when unknown).
    pub modified_ms: u64,
}

/// One artifact as reported by the `Artifact` broker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactInfo {
    /// Artifact id.
    pub artifact_id: String,
    /// Active version.
    pub version: String,
    /// Kind.
    pub kind: WireArtifactKind,
    /// Hex SHA-256 of the active object.
    pub sha256: String,
    /// Size in bytes.
    pub size: u64,
}

/// Structured broker error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrokerErrorCode {
    /// The request was denied by policy (or the emergency stop is active).
    Denied,
    /// The requested object does not exist.
    NotFound,
    /// The request exceeded a size limit.
    SizeExceeded,
    /// The path/origin/target was outside the granted scope.
    InvalidTarget,
    /// The capability is not declared in the plugin's manifest.
    NotDeclared,
    /// The host could not fulfill the request right now.
    Unavailable,
    /// An internal host error occurred.
    Internal,
}

/// Requests a plugin sends to a broker passenger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum BrokerRequest {
    // ── File broker ────────────────────────────────────────────────────
    /// Reads a file through an approved logical slot.
    FileRead {
        /// Path relative to the approved slot (or the slot name alone).
        path: String,
        /// Byte cap for the read (host caps anyway).
        max_bytes: Option<u64>,
    },
    /// Writes a file through an approved writable slot.
    FileWrite {
        /// Path relative to the approved slot.
        path: String,
        /// Bytes to write.
        data: Vec<u8>,
        /// Create the file when absent (fail when false and absent).
        create: bool,
        /// Truncate an existing file to `data.len()`.
        truncate: bool,
    },
    /// Deletes a file through an approved writable slot.
    FileDelete {
        /// Path relative to the approved slot.
        path: String,
        /// Delete directories recursively (equivalent to `remove_dir_all`)
        /// instead of removing a single file.
        #[serde(default)]
        recursive: bool,
    },
    /// Creates a directory through an approved writable slot.
    FileCreateDir {
        /// Path relative to the approved slot.
        path: String,
        /// Create missing parent directories (`create_dir_all`).
        #[serde(default)]
        recursive: bool,
    },
    /// Lists a directory through an approved readable slot.
    FileList {
        /// Path relative to the approved slot.
        path: String,
    },
    /// Stats a path through an approved slot.
    FileStat {
        /// Path relative to the approved slot.
        path: String,
    },
    /// Moves/renames within one approved writable slot.
    FileMove {
        /// Source path.
        from: String,
        /// Destination path.
        to: String,
    },
    /// Saves a completed temp download into an approved writable slot.
    FileSaveDownload {
        /// Temp id returned by [`BrokerResponse::NetworkFetchToTempOk`].
        temp_id: String,
        /// Destination path relative to the approved slot.
        dest_path: String,
        /// Conflict handling (no automatic overwrite).
        conflict: ConflictMode,
    },

    // ── Network broker ─────────────────────────────────────────────────
    /// Fetches a URL with the host's approved origins, returning the body.
    NetworkFetch {
        /// HTTP method.
        method: HttpMethod,
        /// Absolute URL (https by default; http needs a separate approval).
        url: String,
        /// Extra headers (host strips authorization-like headers it did not
        /// inject).
        headers: Vec<(String, String)>,
        /// Name of a host-owned credential to inject as
        /// `Authorization: Bearer <value>` at request time. The plugin only
        /// names the key; the host resolves the value, gates
        /// `CredentialUse`, and never returns it to the plugin.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credential: Option<String>,
        /// Header the credential is injected into. Defaults to
        /// `authorization` (with a `Bearer ` prefix); other headers (e.g.
        /// `x-api-key`) receive the raw value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credential_header: Option<String>,
        /// Request body (JSON/form payloads). The caller must set a matching
        /// `Content-Type` header; the host never interprets the body.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<Vec<u8>>,
        /// Byte cap (host caps anyway).
        max_bytes: Option<u64>,
    },
    /// Fetches a URL into the host's temp area for later
    /// [`BrokerRequest::FileSaveDownload`].
    NetworkFetchToTemp {
        /// Absolute URL.
        url: String,
        /// Byte cap (host caps anyway).
        max_bytes: Option<u64>,
    },
    /// Streams a URL response as frames (`StreamStart`, `StreamChunk`,
    /// `StreamEnd`) instead of buffering it whole. Same gates as
    /// [`BrokerRequest::NetworkFetch`]: SSRF, origin approval, redirect
    /// re-validation, size cap.
    NetworkFetchStream {
        /// HTTP method.
        method: HttpMethod,
        /// Absolute URL.
        url: String,
        /// Extra headers (authorization-like headers are stripped).
        headers: Vec<(String, String)>,
        /// Name of a host-owned credential to inject as
        /// `Authorization: Bearer <value>` at request time (same rules as
        /// [`BrokerRequest::NetworkFetch::credential`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credential: Option<String>,
        /// Header the credential is injected into (same rules as
        /// [`BrokerRequest::NetworkFetch::credential_header`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credential_header: Option<String>,
        /// Request body (form/JSON payloads).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<Vec<u8>>,
        /// Byte cap for the whole stream.
        max_bytes: Option<u64>,
    },

    // ── Process broker ─────────────────────────────────────────────────
    /// Spawns a child process with the host's sandbox and limits.
    ProcessSpawn {
        /// Program + arguments (absolute path or name resolved by the host).
        argv: Vec<String>,
        /// Working directory (host-resolved, default = plugin temp dir).
        cwd: Option<String>,
        /// Additional environment (host filters dangerous names).
        env: Vec<(String, String)>,
        /// Timeout in milliseconds; `0` = host default.
        timeout_ms: u64,
        /// Output byte cap.
        max_output_bytes: u64,
    },
    /// Sends a signal to a previously spawned process.
    ProcessSignal {
        /// Host-assigned pid.
        pid: u32,
        /// Signal number (`15` = SIGTERM, `9` = SIGKILL).
        signal: u32,
    },

    // ── Credential broker ──────────────────────────────────────────────
    /// Reads a credential value (key name only is ever audited).
    CredentialGet {
        /// Credential key declared in the plugin config/schema.
        key: String,
    },
    /// Lists the credential keys this plugin may use.
    CredentialListKeys,

    // ── Artifact broker ────────────────────────────────────────────────
    /// Resolves an artifact requirement to the installed catalog target.
    ArtifactResolve {
        /// Artifact id.
        artifact_id: String,
        /// Optional pinned version.
        version: Option<String>,
    },
    /// Installs (or updates) an artifact through the signed catalog.
    ArtifactInstall {
        /// Artifact id.
        artifact_id: String,
        /// Target version.
        version: String,
    },
    /// Rolls the active artifact back one generation.
    ArtifactRollback {
        /// Artifact id.
        artifact_id: String,
    },
    /// Lists installed artifacts.
    ArtifactList,
    /// Forces a catalog re-fetch (manual refresh); re-verifies signature,
    /// expiry, and rollback rules before replacing the cached metadata.
    ArtifactRefresh,

    // ── Platform broker ────────────────────────────────────────────────
    /// Current wall-clock time.
    PlatformNow,
    /// Host locale/language.
    PlatformLocale,
    /// Opens a URL in the user's browser (subject to `Platform` approval).
    PlatformOpenExternal {
        /// URL to open.
        url: String,
    },
}

/// Broker responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum BrokerResponse {
    /// [`BrokerRequest::FileRead`] succeeded.
    FileReadOk {
        /// File bytes (possibly truncated at the cap).
        data: Vec<u8>,
        /// Full file size before truncation.
        size: u64,
        /// Whether `data` was truncated by the cap.
        truncated: bool,
    },
    /// [`BrokerRequest::FileWrite`] succeeded.
    FileWriteOk {
        /// Bytes on disk after the write.
        size: u64,
    },
    /// [`BrokerRequest::FileDelete`] succeeded.
    FileDeleteOk,
    /// [`BrokerRequest::FileCreateDir`] succeeded.
    FileCreateDirOk,
    /// [`BrokerRequest::FileList`] succeeded.
    FileListOk {
        /// Sorted entries.
        entries: Vec<FileEntry>,
    },
    /// [`BrokerRequest::FileStat`] succeeded.
    FileStatOk {
        /// Entry, or `None` when absent.
        entry: Option<FileEntry>,
    },
    /// [`BrokerRequest::FileMove`] succeeded.
    FileMoveOk,
    /// [`BrokerRequest::FileSaveDownload`] succeeded.
    FileSaveDownloadOk {
        /// Final saved path (after conflict renaming).
        path: String,
        /// Hex SHA-256 of the saved bytes.
        sha256: String,
        /// Saved size.
        size: u64,
    },
    /// [`BrokerRequest::NetworkFetch`] succeeded.
    NetworkFetchOk {
        /// HTTP status.
        status: u16,
        /// Response headers (hop-final).
        headers: Vec<(String, String)>,
        /// Response body (size-capped).
        body: Vec<u8>,
    },
    /// [`BrokerRequest::NetworkFetchToTemp`] succeeded.
    NetworkFetchToTempOk {
        /// Temp id for [`BrokerRequest::FileSaveDownload`].
        temp_id: String,
        /// Final URL after redirects.
        final_url: String,
        /// Downloaded size.
        size: u64,
        /// Hex SHA-256 of the downloaded bytes.
        sha256: String,
        /// Content type, when known.
        mime: Option<String>,
    },
    /// First frame of a [`BrokerRequest::NetworkFetchStream`] response.
    StreamStart {
        /// HTTP status.
        status: u16,
        /// Response headers (authorization/cookie headers stripped).
        headers: Vec<(String, String)>,
    },
    /// Body chunk of a streamed response.
    StreamChunk {
        /// Raw bytes.
        data: Vec<u8>,
    },
    /// Terminal frame of a streamed response.
    StreamEnd,
    /// [`BrokerRequest::ProcessSpawn`] succeeded.
    ProcessSpawnOk {
        /// Host-assigned pid.
        pid: u32,
        /// Exit code when the process finished, else `None`.
        exit_code: Option<i32>,
        /// Captured stdout (size-capped).
        stdout: String,
        /// Captured stderr (size-capped).
        stderr: String,
    },
    /// [`BrokerRequest::ProcessSignal`] succeeded.
    ProcessSignalOk,
    /// [`BrokerRequest::CredentialGet`] succeeded.
    CredentialGetOk {
        /// The credential value.
        value: String,
    },
    /// [`BrokerRequest::CredentialListKeys`] succeeded.
    CredentialListKeysOk {
        /// Key names only (never values).
        keys: Vec<String>,
    },
    /// [`BrokerRequest::ArtifactResolve`] succeeded.
    ArtifactResolveOk {
        /// Resolved artifact.
        artifact: ArtifactInfo,
    },
    /// [`BrokerRequest::ArtifactInstall`] succeeded.
    ArtifactInstallOk {
        /// Newly active artifact.
        artifact: ArtifactInfo,
    },
    /// [`BrokerRequest::ArtifactRollback`] succeeded.
    ArtifactRollbackOk {
        /// Restored artifact.
        artifact: ArtifactInfo,
    },
    /// [`BrokerRequest::ArtifactList`] succeeded.
    ArtifactListOk {
        /// Installed artifacts, sorted by id.
        artifacts: Vec<ArtifactInfo>,
    },
    /// [`BrokerRequest::ArtifactRefresh`] succeeded.
    ArtifactRefreshOk {
        /// Version of the freshly verified catalog metadata.
        catalog_version: u64,
    },
    /// [`BrokerRequest::PlatformNow`] succeeded.
    PlatformNowOk {
        /// Unix milliseconds.
        unix_ms: u64,
    },
    /// [`BrokerRequest::PlatformLocale`] succeeded.
    PlatformLocaleOk {
        /// BCP-47 language tag.
        language: String,
    },
    /// [`BrokerRequest::PlatformOpenExternal`] succeeded.
    PlatformOpenExternalOk,
    /// Any broker request failed.
    Error {
        /// Structured code.
        code: BrokerErrorCode,
        /// Human-readable detail.
        message: String,
    },
}

impl BrokerResponse {
    /// Builds a [`BrokerResponse::Error`].
    #[must_use]
    pub fn error(code: BrokerErrorCode, message: impl Into<String>) -> Self {
        Self::Error {
            code,
            message: message.into(),
        }
    }
}

/// Host-side handler for one plugin's broker sessions.
///
/// The host service dispatches every request on an opened broker passenger
/// (`file`, `network`, `process`, `credential`, `artifact`, `platform`) to
/// this trait with the authenticated plugin name. Implementations must fail
/// closed: unverifiable manifests, missing grants, and unresolved approvals
/// produce [`BrokerResponse::Error`].
#[async_trait::async_trait]
pub trait BrokerHandler: Send + Sync {
    /// Handles one broker request from `plugin`.
    async fn handle(&self, plugin: &str, request: BrokerRequest) -> BrokerResponse;

    /// Handles a request that produces multiple response frames.
    ///
    /// The default writes the single [`handle`](Self::handle) response;
    /// streaming services override this to write `StreamStart`/`StreamChunk`/
    /// `StreamEnd` frames through `sink`.
    async fn handle_stream(
        &self,
        plugin: &str,
        request: BrokerRequest,
        sink: &mut (dyn BrokerSink + Send),
    ) -> std::io::Result<()> {
        let response = self.handle(plugin, request).await;
        sink.write(&response).await
    }

    /// Serves a `WebSocket` passenger session on `stream`.
    ///
    /// The host-service server has already authenticated the plugin and
    /// written `OpenAck`; the first frame on the stream is
    /// [`WebSocketRequest::Open`](crate::ws::WebSocketRequest::Open). The
    /// default rejects the session.
    async fn serve_ws(
        &self,
        _plugin: &str,
        mut stream: crate::transport::IpcStream,
    ) -> std::io::Result<()> {
        crate::host_service::write_framed_json(
            &mut stream,
            &crate::ws::WebSocketResponse::Error {
                status: None,
                message: "WebSocket passenger is not implemented by this handler".to_string(),
            },
        )
        .await
    }
}

/// Frame sink for streaming broker responses (the host-service session
/// loop implements this over the framed socket).
#[async_trait::async_trait]
pub trait BrokerSink: Send {
    /// Writes one response frame.
    async fn write(&mut self, response: &BrokerResponse) -> std::io::Result<()>;
}

/// Convenience alias for sharing a broker handler.
pub type SharedBrokerHandler = Arc<dyn BrokerHandler>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broker_request_round_trips_all_services() {
        let requests = vec![
            BrokerRequest::FileRead {
                path: "notes.txt".to_string(),
                max_bytes: Some(1024),
            },
            BrokerRequest::FileWrite {
                path: "out.txt".to_string(),
                data: b"hi".to_vec(),
                create: true,
                truncate: true,
            },
            BrokerRequest::NetworkFetch {
                method: HttpMethod::Get,
                url: "https://example.com".to_string(),
                headers: vec![],
                credential: Some("api_key".to_string()),
                credential_header: None,
                body: None,
                max_bytes: None,
            },
            BrokerRequest::ProcessSpawn {
                argv: vec!["sh".to_string()],
                cwd: None,
                env: vec![],
                timeout_ms: 0,
                max_output_bytes: 1024,
            },
            BrokerRequest::CredentialGet {
                key: "api_key".to_string(),
            },
            BrokerRequest::ArtifactResolve {
                artifact_id: "llama".to_string(),
                version: None,
            },
            BrokerRequest::PlatformNow,
        ];
        for request in requests {
            let json = serde_json::to_value(&request).expect("serialize");
            let back: BrokerRequest = serde_json::from_value(json).expect("deserialize");
            assert_eq!(request, back);
        }
    }

    #[test]
    fn broker_response_error_helper() {
        let response = BrokerResponse::error(BrokerErrorCode::Denied, "no");
        assert!(matches!(
            response,
            BrokerResponse::Error {
                code: BrokerErrorCode::Denied,
                ..
            }
        ));
    }
}
