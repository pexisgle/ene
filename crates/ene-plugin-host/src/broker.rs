//! Host-mediated broker channel: the only way plugins touch user files, the
//! network, processes, credentials, and executable artifacts.
//!
//! Every request passes four gates, in order:
//!
//! 1. **Identity** — the host service dispatches with the plugin name bound
//!    to the authenticated token; a plugin cannot spoof another's identity.
//! 2. **Manifest layer** — the capability must be declared (host service,
//!    permission entry); undeclared capabilities are rejected even when a
//!    policy says `Allow`.
//! 3. **Approval layer** — per-plugin override → global policy → `Ask`.
//!    `Ask` fails safe to denial when no interactive responder is attached.
//! 4. **Mandatory constraints** — SSRF blocks, path containment, size caps,
//!    digest verification, implicit-download bans. These hold regardless of
//!    any approval.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use ene_approval::{
    ApprovalCategory, ApprovalMode, ApprovalPolicy, ApprovalResolver, AuditLog, AuditLogEntry,
    PluginApprovalPolicy, PluginManifest, ResolvedMode, SignedManifest,
};
use ene_artifact::{
    ArtifactInstaller, ArtifactKind, ArtifactTarget, CatalogVerifier, Downloader, InstallerConfig,
    TrustedCatalogKeys,
};
use ene_plugin_proto::{
    ArtifactInfo, BrokerErrorCode, BrokerHandler, BrokerRequest, BrokerResponse, ConflictMode,
    FileEntry, HttpMethod, WireArtifactKind,
};
use parking_lot::Mutex;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::config::{ArtifactConfig, DownloadConfig, FsGrantConfig, PluginConfig, PluginEntry};
use crate::manifest::{FsGrant, ManifestStore, canonical_within, resolve_grant_path};

/// Interactive approval responder: resolves `Ask` requests by showing the
/// user a confirmation. Headless hosts leave the hub without a responder,
/// which fails `Ask` safe to denial.
#[async_trait]
pub trait ApprovalResponder: Send + Sync {
    /// Presents one approval request and returns the user's decision.
    async fn request(&self, plugin: &str, category: ApprovalCategory, target: &str)
    -> ResolvedMode;
}

/// Fail-closed error used inside the hub; converted to
/// [`BrokerResponse::Error`] at the boundary.
#[derive(Debug)]
struct BrokerError {
    code: BrokerErrorCode,
    message: String,
}

impl BrokerError {
    fn new(code: BrokerErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn denied(message: impl Into<String>) -> Self {
        Self::new(BrokerErrorCode::Denied, message)
    }
}

/// One plugin's broker state: verified manifest, grants, credentials, and
/// live spawned processes.
struct PluginState {
    manifest: Option<PluginManifest>,
    digest: Option<String>,
    fs_grants: Vec<FsGrant>,
    credentials: BTreeMap<String, String>,
    processes: Mutex<HashMap<u32, String>>,
}

/// Host-side broker hub.
pub struct BrokerHub {
    plugins: HashMap<String, PluginState>,
    global: ApprovalPolicy,
    plugin_approval: std::collections::BTreeMap<String, PluginApprovalPolicy>,
    audit: Option<AuditLog>,
    artifact: Option<ArtifactServices>,
    download: DownloadServices,
    http: reqwest::Client,
    responder: parking_lot::RwLock<Option<Arc<dyn ApprovalResponder>>>,
}

struct ArtifactServices {
    installer: ArtifactInstaller,
    verifier: CatalogVerifier,
    downloader: Downloader,
    config: ArtifactConfig,
    /// Cached verified catalog plus its fetch timestamp (Unix ms). The
    /// cache expires after `ArtifactConfig::refresh_hours` so a revoked or
    /// rolled-back artifact cannot stay installable for the process
    /// lifetime.
    catalog: Mutex<Option<(ene_artifact::CatalogMetadata, u64)>>,
}

struct DownloadServices {
    temp_dir: PathBuf,
    config: DownloadConfig,
}

impl BrokerHub {
    /// Builds the hub from plugin configuration.
    ///
    /// Returns `None` when the plugin system is disabled.
    pub fn from_config(config: &ene_config::EneConfig) -> Option<Arc<Self>> {
        let plugin_config = config.get_section::<PluginConfig>().unwrap_or_default();
        if !plugin_config.enabled {
            return None;
        }
        // The host uses the ring TLS provider (no aws-lc native build, and
        // the ring provider crosses to Windows cleanly). Installing it once
        // is idempotent; a second install attempt is ignored.
        drop(rustls::crypto::ring::default_provider().install_default());
        let audit = Some(AuditLog::new(
            plugin_config.audit_log_path.clone().unwrap_or_else(|| {
                ene_config::app_data_dir()
                    .join("audit")
                    .join("plugin-approval.jsonl")
                    .to_string_lossy()
                    .into_owned()
            }),
        ));
        let manifest_store = ManifestStore::new(&plugin_config.trusted_publishers);
        let mut plugins = HashMap::new();
        for (name, entry) in &plugin_config.list {
            if !entry.enable {
                continue;
            }
            plugins.insert(
                name.clone(),
                build_plugin_state(name, entry, &manifest_store),
            );
        }
        seed_provider_credentials(config, &plugin_config, &mut plugins);
        let artifact = build_artifact_services(&plugin_config.artifact);
        let download = DownloadServices {
            temp_dir: ene_config::app_data_dir().join("tmp").join("downloads"),
            config: plugin_config.download.clone(),
        };
        let http = match reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("ene-host/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_mins(2))
            .build()
        {
            Ok(client) => client,
            Err(e) => {
                tracing::warn!(error = %e, "failed to build broker HTTP client");
                return None;
            }
        };
        let hub = Arc::new(Self {
            plugins,
            global: plugin_config.approval.clone(),
            plugin_approval: plugin_config.plugin_approval.clone(),
            audit,
            artifact,
            download,
            http,
            responder: parking_lot::RwLock::new(None),
        });
        Some(hub)
    }

    /// Attaches an interactive responder for `Ask` resolutions.
    #[must_use]
    pub fn with_approval_responder(
        self: &Arc<Self>,
        responder: Arc<dyn ApprovalResponder>,
    ) -> Arc<Self> {
        *self.responder.write() = Some(responder);
        Arc::clone(self)
    }
}

/// Seeds broker credentials from the resolved `ai.providers.<kind>.api_key`
/// so provider plugins can reference the key by name (`credential:
/// "api_key"`) and the host injects it at request time. Mirrors the
/// factory's trust gate: only built-in or explicitly listed plugins receive
/// the key, and an explicit `plugins.list.<name>.credentials` entry wins.
fn seed_provider_credentials(
    config: &ene_config::EneConfig,
    plugin_config: &PluginConfig,
    plugins: &mut HashMap<String, PluginState>,
) {
    let ai_config = config.get_section::<ene_ai::AiConfig>().unwrap_or_default();
    for def in ai_config.providers.values() {
        let key = def.api_key.resolve_api_key();
        if key.is_empty() {
            continue;
        }
        for (name, state) in &mut *plugins {
            let trusted =
                crate::manager::is_builtin_plugin(name) || plugin_config.list.contains_key(name);
            if !trusted || !crate::factory::provider_def_kind_matches(def, name) {
                continue;
            }
            state
                .credentials
                .entry("api_key".to_string())
                .or_insert_with(|| key.clone());
        }
    }
}

fn build_plugin_state(
    name: &str,
    entry: &PluginEntry,
    manifest_store: &ManifestStore,
) -> PluginState {
    let (manifest, digest) = match &entry.manifest {
        Some(signed) => {
            let digest = ManifestStore::digest(signed);
            match manifest_store.verify(signed, name) {
                Ok(manifest) => (Some(manifest), Some(digest)),
                Err(e) => {
                    tracing::error!(plugin = %name, error = %e, "manifest verification failed");
                    (None, None)
                }
            }
        }
        None => {
            // Built-ins: embed the manifest if the entry has none. A
            // discovered binary with no entry gets no manifest at all.
            if let Some(manifest) = crate::manifest::builtin_manifest(name) {
                let signed = SignedManifest {
                    payload: ene_approval::canonical_manifest_bytes(&manifest).unwrap_or_default(),
                    signature: None,
                    key_id: None,
                };
                (Some(manifest), Some(ManifestStore::digest(&signed)))
            } else {
                (None, None)
            }
        }
    };
    let fs_grants = entry
        .fs_grants
        .iter()
        .filter_map(|grant: &FsGrantConfig| {
            let path = PathBuf::from(&grant.path);
            let canonical = path.canonicalize().unwrap_or(path);
            if !canonical.is_dir() {
                tracing::warn!(
                    plugin = %name,
                    slot = %grant.slot,
                    path = %canonical.display(),
                    "ignoring fs grant: path is not a directory"
                );
                return None;
            }
            Some(FsGrant {
                slot: grant.slot.clone(),
                path: canonical,
                read: grant.read,
                write: grant.write,
            })
        })
        .collect();
    PluginState {
        manifest,
        digest,
        fs_grants,
        credentials: entry.credentials.clone(),
        processes: Mutex::new(HashMap::new()),
    }
}

fn build_artifact_services(config: &ArtifactConfig) -> Option<ArtifactServices> {
    if !config.enabled {
        return None;
    }
    let root = config.root_dir.as_ref().map_or_else(
        || ene_config::app_data_dir().join("artifacts"),
        PathBuf::from,
    );
    let keys = TrustedCatalogKeys::from_hex(
        &config
            .catalog_keys
            .iter()
            .map(|key| (key.key_id.clone(), key.public_key_hex.clone()))
            .collect::<Vec<_>>(),
    )
    .ok()?;
    if config.catalog_url.is_none() || keys.is_empty() {
        tracing::warn!("artifact system enabled but catalog_url/keys missing");
        return None;
    }
    let installer = ArtifactInstaller::new(InstallerConfig {
        cas_root: root.join("cas"),
        state_path: root.join("state.json"),
    })
    .ok()?;
    let downloader = Downloader::new(
        Some(std::time::Duration::from_secs(30)),
        Some(std::time::Duration::from_millis(config.timeout_ms)),
        config.max_redirects,
    )
    .ok()?;
    Some(ArtifactServices {
        installer,
        verifier: CatalogVerifier::new(keys),
        downloader,
        config: config.clone(),
        catalog: Mutex::new(None),
    })
}

#[async_trait]
impl BrokerHandler for BrokerHub {
    async fn handle(&self, plugin: &str, request: BrokerRequest) -> BrokerResponse {
        let Some(state) = self.plugins.get(plugin) else {
            return BrokerResponse::error(
                BrokerErrorCode::NotDeclared,
                format!("plugin '{plugin}' has no verified manifest"),
            );
        };
        match self.dispatch(plugin, state, request).await {
            Ok(response) => response,
            Err(e) => BrokerResponse::error(e.code, e.message),
        }
    }

    async fn handle_stream(
        &self,
        plugin: &str,
        request: BrokerRequest,
        sink: &mut (dyn ene_plugin_proto::BrokerSink + Send),
    ) -> std::io::Result<()> {
        let result = match request {
            BrokerRequest::NetworkFetchStream {
                method,
                url,
                headers,
                credential,
                body,
                max_bytes,
            } => {
                let Some(state) = self.plugins.get(plugin) else {
                    return sink
                        .write(&BrokerResponse::error(
                            BrokerErrorCode::NotDeclared,
                            format!("plugin '{plugin}' has no verified manifest"),
                        ))
                        .await;
                };
                if let Err(e) = Self::require_service(state, "network") {
                    return sink.write(&BrokerResponse::error(e.code, e.message)).await;
                }
                let cap = max_bytes.unwrap_or(self.download.config.max_bytes);
                self.fetch_stream(
                    plugin,
                    state,
                    method,
                    &url,
                    &headers,
                    credential.as_deref(),
                    body,
                    cap,
                    sink,
                )
                .await
            }
            other => {
                let response = self.handle(plugin, other).await;
                return sink.write(&response).await;
            }
        };
        match result {
            Ok(()) => Ok(()),
            Err(e) => sink.write(&BrokerResponse::error(e.code, e.message)).await,
        }
    }
}

impl BrokerHub {
    async fn dispatch(
        &self,
        plugin: &str,
        state: &PluginState,
        request: BrokerRequest,
    ) -> Result<BrokerResponse, BrokerError> {
        Self::require_service(state, service_of(&request))?;
        match request {
            // ── File broker ─────────────────────────────────────────────
            BrokerRequest::FileRead { path, max_bytes } => {
                self.file_read(plugin, state, &path, max_bytes).await
            }
            BrokerRequest::FileWrite {
                path,
                data,
                create,
                truncate,
            } => {
                self.file_write(plugin, state, &path, &data, create, truncate)
                    .await
            }
            BrokerRequest::FileDelete { path, recursive } => {
                self.file_delete(plugin, state, &path, recursive).await
            }
            BrokerRequest::FileCreateDir { path, recursive } => {
                self.file_create_dir(plugin, state, &path, recursive).await
            }
            BrokerRequest::FileList { path } => self.file_list(plugin, state, &path).await,
            BrokerRequest::FileStat { path } => self.file_stat(plugin, state, &path).await,
            BrokerRequest::FileMove { from, to } => self.file_move(plugin, state, &from, &to).await,
            BrokerRequest::FileSaveDownload {
                temp_id,
                dest_path,
                conflict,
            } => {
                self.file_save_download(plugin, state, &temp_id, &dest_path, conflict)
                    .await
            }
            // ── Network broker ──────────────────────────────────────────
            BrokerRequest::NetworkFetch {
                method,
                url,
                headers,
                credential,
                body,
                max_bytes,
            } => {
                self.network_fetch(
                    plugin,
                    state,
                    method,
                    &url,
                    &headers,
                    credential.as_deref(),
                    body,
                    max_bytes,
                )
                .await
            }
            BrokerRequest::NetworkFetchToTemp { url, max_bytes } => {
                self.network_fetch_to_temp(plugin, state, &url, max_bytes)
                    .await
            }
            BrokerRequest::NetworkFetchStream { .. } => {
                // The session loop routes streaming requests to
                // `handle_stream`; this arm is unreachable through `handle`.
                Err(BrokerError::new(
                    BrokerErrorCode::Internal,
                    "streaming requests must use the streaming handler",
                ))
            }
            // ── Process broker ──────────────────────────────────────────
            BrokerRequest::ProcessSpawn {
                argv,
                cwd,
                env,
                timeout_ms,
                max_output_bytes,
            } => {
                self.process_spawn(plugin, state, argv, cwd, env, timeout_ms, max_output_bytes)
                    .await
            }
            BrokerRequest::ProcessSignal { pid, signal } => {
                std::future::ready(Self::process_signal(state, pid, signal)).await
            }
            // ── Credential broker ───────────────────────────────────────
            BrokerRequest::CredentialGet { key } => self.credential_get(plugin, state, &key).await,
            BrokerRequest::CredentialListKeys => self.credential_list_keys(plugin, state).await,
            // ── Artifact broker ─────────────────────────────────────────
            BrokerRequest::ArtifactResolve {
                artifact_id,
                version,
            } => {
                self.artifact_resolve(plugin, state, &artifact_id, version)
                    .await
            }
            BrokerRequest::ArtifactInstall {
                artifact_id,
                version,
            } => {
                self.artifact_install(plugin, state, &artifact_id, &version)
                    .await
            }
            BrokerRequest::ArtifactRollback { artifact_id } => {
                std::future::ready(self.artifact_rollback(&artifact_id)).await
            }
            BrokerRequest::ArtifactList => std::future::ready(self.artifact_list()).await,
            BrokerRequest::ArtifactRefresh => self.artifact_refresh().await,
            // ── Platform broker ─────────────────────────────────────────
            BrokerRequest::PlatformNow => Ok(BrokerResponse::PlatformNowOk {
                unix_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis().try_into().unwrap_or(u64::MAX)),
            }),
            BrokerRequest::PlatformLocale => Ok(BrokerResponse::PlatformLocaleOk {
                language: sys_locale::get_locale().unwrap_or_else(|| "en-US".to_string()),
            }),
            BrokerRequest::PlatformOpenExternal { url } => {
                self.platform_open_external(plugin, state, &url).await
            }
        }
    }

    fn require_service(state: &PluginState, service: &str) -> Result<(), BrokerError> {
        let declared = state
            .manifest
            .as_ref()
            .is_some_and(|m| m.host_services.iter().any(|s| s == service));
        if declared {
            Ok(())
        } else {
            Err(BrokerError::new(
                BrokerErrorCode::NotDeclared,
                format!("host service '{service}' is not declared in the plugin manifest"),
            ))
        }
    }

    async fn approve(
        &self,
        plugin: &str,
        state: &PluginState,
        category: ApprovalCategory,
        target: &str,
    ) -> Result<(), BrokerError> {
        let manifest_allows = state.manifest.as_ref().is_some_and(|m| {
            m.permissions
                .iter()
                .any(|p| p.category == category && p.max != ApprovalMode::Deny)
        });
        if !manifest_allows {
            self.audit(
                plugin,
                state,
                category,
                target,
                "manifest_layer",
                "capability not declared in the signed manifest",
                ResolvedMode::Deny,
            );
            return Err(BrokerError::new(
                BrokerErrorCode::NotDeclared,
                format!("category {category:?} is not declared in the plugin manifest"),
            ));
        }
        let resolution =
            ApprovalResolver::new(&self.global, &self.plugin_approval).resolve(plugin, category);
        self.audit(
            plugin,
            state,
            category,
            target,
            resolution.reason.label(),
            resolution.rule,
            resolution.mode,
        );
        match resolution.mode {
            ResolvedMode::Allow => Ok(()),
            ResolvedMode::Deny => Err(BrokerError::denied(format!(
                "denied by policy: {}",
                resolution.rule
            ))),
            ResolvedMode::Ask => {
                let responder = self.responder.read().clone();
                let decision = if let Some(responder) = responder {
                    responder.request(plugin, category, target).await
                } else {
                    ResolvedMode::Deny
                };
                self.audit(
                    plugin,
                    state,
                    category,
                    target,
                    "interactive_confirmation",
                    "user responded to the confirmation dialog",
                    decision,
                );
                if decision.allows() {
                    Ok(())
                } else {
                    Err(BrokerError::denied("denied by the user"))
                }
            }
        }
    }

    fn audit(
        &self,
        plugin: &str,
        state: &PluginState,
        category: ApprovalCategory,
        target: &str,
        reason: &str,
        rule: &str,
        decision: ResolvedMode,
    ) {
        let Some(audit) = &self.audit else {
            return;
        };
        let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis().try_into().unwrap_or(u64::MAX));
        let entry = AuditLogEntry {
            ts_ms,
            plugin: plugin.to_string(),
            manifest_digest: state.digest.clone(),
            category,
            target: target.to_string(),
            reason: reason.to_string(),
            rule: rule.to_string(),
            decision,
        };
        if let Err(e) = audit.record(&entry) {
            tracing::warn!(error = %e, "failed to write approval audit log");
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().try_into().unwrap_or(u64::MAX))
}

fn service_of(request: &BrokerRequest) -> &'static str {
    match request {
        BrokerRequest::FileRead { .. }
        | BrokerRequest::FileWrite { .. }
        | BrokerRequest::FileDelete { .. }
        | BrokerRequest::FileCreateDir { .. }
        | BrokerRequest::FileList { .. }
        | BrokerRequest::FileStat { .. }
        | BrokerRequest::FileMove { .. }
        | BrokerRequest::FileSaveDownload { .. } => "file",
        BrokerRequest::NetworkFetch { .. }
        | BrokerRequest::NetworkFetchToTemp { .. }
        | BrokerRequest::NetworkFetchStream { .. } => "network",
        BrokerRequest::ProcessSpawn { .. } | BrokerRequest::ProcessSignal { .. } => "process",
        BrokerRequest::CredentialGet { .. } | BrokerRequest::CredentialListKeys => "credential",
        BrokerRequest::ArtifactResolve { .. }
        | BrokerRequest::ArtifactInstall { .. }
        | BrokerRequest::ArtifactRollback { .. }
        | BrokerRequest::ArtifactList
        | BrokerRequest::ArtifactRefresh => "artifact",
        BrokerRequest::PlatformNow
        | BrokerRequest::PlatformLocale
        | BrokerRequest::PlatformOpenExternal { .. } => "platform",
    }
}

// ── File broker ─────────────────────────────────────────────────────────

/// Resolves a file-broker path to a canonical target inside a grant.
///
/// Slot-relative paths (`workspace/notes.txt`) resolve through the named
/// grant; absolute paths are matched against grants by canonical
/// containment (the fs plugin keeps its tool contract of absolute paths).
fn resolve_file_target(
    state: &PluginState,
    path: &str,
    need_write: bool,
) -> Result<PathBuf, BrokerError> {
    if Path::new(path).is_absolute() {
        crate::manifest::resolve_grant_abs(&state.fs_grants, Path::new(path), need_write)
            .map_err(|e| BrokerError::denied(e.to_string()))
    } else {
        let (grant, target) = resolve_grant_path(&state.fs_grants, path, need_write)
            .map_err(|e| BrokerError::denied(e.to_string()))?;
        canonical_within(&grant.path, &target)
            .ok_or_else(|| BrokerError::denied("path escapes the granted directory"))
    }
}

impl BrokerHub {
    async fn file_read(
        &self,
        plugin: &str,
        state: &PluginState,
        path: &str,
        max_bytes: Option<u64>,
    ) -> Result<BrokerResponse, BrokerError> {
        let canonical = resolve_file_target(state, path, false)?;
        self.approve(
            plugin,
            state,
            ApprovalCategory::FsRead,
            &canonical.to_string_lossy(),
        )
        .await?;
        let cap = max_bytes.unwrap_or(50 * 1024 * 1024);
        let metadata = std::fs::metadata(&canonical)
            .map_err(|e| BrokerError::new(BrokerErrorCode::NotFound, e.to_string()))?;
        if !metadata.is_file() {
            return Err(BrokerError::new(
                BrokerErrorCode::NotFound,
                "not a regular file",
            ));
        }
        let size = metadata.len();
        let read_len = usize::try_from(size.min(cap)).unwrap_or(usize::MAX);
        let mut data = vec![0u8; read_len];
        let mut file = std::fs::File::open(&canonical)
            .map_err(|e| BrokerError::new(BrokerErrorCode::NotFound, e.to_string()))?;
        use std::io::Read;
        file.read_exact(&mut data)
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        Ok(BrokerResponse::FileReadOk {
            data,
            size,
            truncated: size > cap,
        })
    }

    async fn file_write(
        &self,
        plugin: &str,
        state: &PluginState,
        path: &str,
        data: &[u8],
        create: bool,
        truncate: bool,
    ) -> Result<BrokerResponse, BrokerError> {
        let canonical = resolve_file_target(state, path, true)?;
        let exists = canonical.exists();
        if !exists && !create {
            return Err(BrokerError::new(
                BrokerErrorCode::NotFound,
                "file does not exist and create=false",
            ));
        }
        let category = if exists {
            ApprovalCategory::FsModify
        } else {
            ApprovalCategory::FsCreate
        };
        self.approve(plugin, state, category, &canonical.to_string_lossy())
            .await?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true);
        if truncate {
            options.truncate(true);
        }
        if create {
            options.create(true);
        }
        let mut file = options
            .open(&canonical)
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        use std::io::Write;
        file.write_all(data)
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        Ok(BrokerResponse::FileWriteOk {
            size: u64::try_from(data.len()).unwrap_or(u64::MAX),
        })
    }

    async fn file_delete(
        &self,
        plugin: &str,
        state: &PluginState,
        path: &str,
        recursive: bool,
    ) -> Result<BrokerResponse, BrokerError> {
        let canonical = resolve_file_target(state, path, true)?;
        self.approve(
            plugin,
            state,
            ApprovalCategory::FsDelete,
            &canonical.to_string_lossy(),
        )
        .await?;
        let result = if recursive {
            std::fs::remove_dir_all(&canonical)
        } else {
            std::fs::remove_file(&canonical)
        };
        result.map_err(|e| BrokerError::new(BrokerErrorCode::NotFound, e.to_string()))?;
        Ok(BrokerResponse::FileDeleteOk)
    }

    async fn file_create_dir(
        &self,
        plugin: &str,
        state: &PluginState,
        path: &str,
        recursive: bool,
    ) -> Result<BrokerResponse, BrokerError> {
        let canonical = resolve_file_target(state, path, true)?;
        self.approve(
            plugin,
            state,
            ApprovalCategory::FsCreate,
            &canonical.to_string_lossy(),
        )
        .await?;
        let result = if recursive {
            std::fs::create_dir_all(&canonical)
        } else {
            std::fs::create_dir(&canonical)
        };
        result.map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        Ok(BrokerResponse::FileCreateDirOk)
    }

    async fn file_list(
        &self,
        plugin: &str,
        state: &PluginState,
        path: &str,
    ) -> Result<BrokerResponse, BrokerError> {
        let canonical = resolve_file_target(state, path, false)?;
        self.approve(
            plugin,
            state,
            ApprovalCategory::FsRead,
            &canonical.to_string_lossy(),
        )
        .await?;
        let mut entries = Vec::new();
        let read_dir = std::fs::read_dir(&canonical)
            .map_err(|e| BrokerError::new(BrokerErrorCode::NotFound, e.to_string()))?;
        for entry in read_dir {
            let entry =
                entry.map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
            // `file_type` does not follow symlinks: a symlinked directory
            // must not be reported as a directory, otherwise plugin-side
            // recursive walks would follow it out of the grant.
            let file_type = entry
                .file_type()
                .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
            let entry_metadata = entry.metadata().ok();
            entries.push(FileEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: entry.path().to_string_lossy().into_owned(),
                is_dir: file_type.is_dir(),
                size: entry_metadata.as_ref().map_or(0, std::fs::Metadata::len),
                modified_ms: entry_metadata
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis().try_into().unwrap_or(u64::MAX)),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(BrokerResponse::FileListOk { entries })
    }

    async fn file_stat(
        &self,
        plugin: &str,
        state: &PluginState,
        path: &str,
    ) -> Result<BrokerResponse, BrokerError> {
        let canonical = resolve_file_target(state, path, false)?;
        self.approve(
            plugin,
            state,
            ApprovalCategory::FsRead,
            &canonical.to_string_lossy(),
        )
        .await?;
        let entry = match std::fs::metadata(&canonical) {
            Ok(metadata) => Some(FileEntry {
                name: canonical.file_name().map_or_else(
                    || canonical.to_string_lossy().into_owned(),
                    |n| n.to_string_lossy().into_owned(),
                ),
                path: canonical.to_string_lossy().into_owned(),
                is_dir: metadata.is_dir(),
                size: metadata.len(),
                modified_ms: metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis().try_into().unwrap_or(u64::MAX)),
            }),
            Err(_) => None,
        };
        Ok(BrokerResponse::FileStatOk { entry })
    }

    async fn file_move(
        &self,
        plugin: &str,
        state: &PluginState,
        from: &str,
        to: &str,
    ) -> Result<BrokerResponse, BrokerError> {
        let source = resolve_file_target(state, from, true)?;
        let target = resolve_file_target(state, to, true)?;
        self.approve(
            plugin,
            state,
            ApprovalCategory::FsModify,
            &format!("{} -> {}", source.display(), target.display()),
        )
        .await?;
        std::fs::rename(&source, &target)
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        Ok(BrokerResponse::FileMoveOk)
    }

    async fn file_save_download(
        &self,
        plugin: &str,
        state: &PluginState,
        temp_id: &str,
        dest_path: &str,
        conflict: ConflictMode,
    ) -> Result<BrokerResponse, BrokerError> {
        let (grant, target) = resolve_grant_path(&state.fs_grants, dest_path, true)
            .map_err(|e| BrokerError::denied(e.to_string()))?;
        let safe_name = sanitize_file_name(
            target
                .file_name()
                .map_or("download", |n| n.to_str().unwrap_or("download")),
        );
        let final_path = grant.path.join(safe_name);
        let canonical = canonical_within(&grant.path, &final_path)
            .ok_or_else(|| BrokerError::denied("path escapes the granted directory"))?;
        if canonical.exists() {
            match conflict {
                ConflictMode::Fail => {
                    return Err(BrokerError::new(
                        BrokerErrorCode::NotFound,
                        "destination exists (no automatic overwrite)",
                    ));
                }
                ConflictMode::Rename => {}
            }
        }
        let destination = uniquify(&canonical);
        self.approve(
            plugin,
            state,
            ApprovalCategory::WebFileSave,
            &format!("temp {temp_id} -> {}", destination.to_string_lossy()),
        )
        .await?;
        let source = self.download.temp_dir.join(temp_id);
        let bytes = std::fs::read(&source)
            .map_err(|e| BrokerError::new(BrokerErrorCode::NotFound, e.to_string()))?;
        std::fs::write(&destination, bytes)
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        drop(std::fs::remove_file(&source));
        Ok(BrokerResponse::FileSaveDownloadOk {
            path: destination.to_string_lossy().into_owned(),
            sha256: ene_artifact::sha256_hex(&std::fs::read(&destination).unwrap_or_default()),
            size: std::fs::metadata(&destination).map_or(0, |m| m.len()),
        })
    }
}

fn sanitize_file_name(name: &str) -> String {
    let mut safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ' | '(' | ')') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() || safe == "." || safe == ".." {
        safe = "download".to_string();
    }
    safe
}

fn uniquify(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .map_or("download", |s| s.to_str().unwrap_or("download"));
    let extension = path.extension().and_then(|e| e.to_str());
    for i in 1..10_000_u32 {
        let candidate = match extension {
            Some(ext) => parent.join(format!("{stem}-{i}.{ext}")),
            None => parent.join(format!("{stem}-{i}")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    path.to_path_buf()
}

// ── Network broker ──────────────────────────────────────────────────────

impl BrokerHub {
    async fn network_fetch(
        &self,
        plugin: &str,
        state: &PluginState,
        method: HttpMethod,
        url: &str,
        headers: &[(String, String)],
        credential: Option<&str>,
        body: Option<Vec<u8>>,
        max_bytes: Option<u64>,
    ) -> Result<BrokerResponse, BrokerError> {
        let cap = max_bytes.unwrap_or(self.download.config.max_bytes);
        let body = self
            .fetch_loop(
                plugin, state, method, url, headers, credential, body, cap, None,
            )
            .await?;
        Ok(BrokerResponse::NetworkFetchOk {
            status: body.status,
            headers: body.headers,
            body: body.bytes,
        })
    }

    async fn network_fetch_to_temp(
        &self,
        plugin: &str,
        state: &PluginState,
        url: &str,
        max_bytes: Option<u64>,
    ) -> Result<BrokerResponse, BrokerError> {
        let cap = max_bytes.unwrap_or(self.download.config.max_bytes);
        std::fs::create_dir_all(&self.download.temp_dir)
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        let temp_id = uuid::Uuid::new_v4().to_string();
        let temp_path = self.download.temp_dir.join(&temp_id);
        let body = self
            .fetch_loop(
                plugin,
                state,
                HttpMethod::Get,
                url,
                &[],
                None,
                None,
                cap,
                Some(&temp_path),
            )
            .await?;
        Ok(BrokerResponse::NetworkFetchToTempOk {
            temp_id,
            final_url: body.final_url,
            size: body.size,
            sha256: body.sha256,
            mime: body.mime,
        })
    }

    /// Builds one validated fetch hop: origin approval, SSRF check with
    /// address pinning, and the request itself.
    async fn build_fetch_hop(
        &self,
        plugin: &str,
        state: &PluginState,
        method: HttpMethod,
        url: &str,
        headers: &[(String, String)],
        credential: Option<&str>,
        body: Option<&Vec<u8>>,
    ) -> Result<FetchHop, BrokerError> {
        let parsed = url::Url::parse(url)
            .map_err(|e| BrokerError::new(BrokerErrorCode::InvalidTarget, e.to_string()))?;
        let scheme = parsed.scheme().to_string();
        if scheme != "https" && scheme != "http" {
            return Err(BrokerError::new(
                BrokerErrorCode::InvalidTarget,
                "only https (or approved http) URLs are supported",
            ));
        }
        let host = parsed
            .host_str()
            .ok_or_else(|| BrokerError::new(BrokerErrorCode::InvalidTarget, "URL has no host"))?;
        let port = parsed.port_or_known_default().unwrap_or(443);
        let origin = format!("{scheme}://{host}:{port}");
        let category = Self::origin_category(state, &origin, &scheme)?;
        self.approve(plugin, state, category, &origin).await?;
        let ssrf = SsrfPolicy::production();
        let ips = ssrf
            .resolve_allowed(host)
            .await
            .map_err(|e| BrokerError::denied(format!("SSRF guard: {e}")))?;
        let Some(ip) = ips.first() else {
            return Err(BrokerError::denied("SSRF guard: no allowed address"));
        };
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .resolve(host, std::net::SocketAddr::new(*ip, port))
            .timeout(std::time::Duration::from_mins(2))
            .build()
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        let mut request = match method {
            HttpMethod::Get => client.get(url),
            HttpMethod::Post => client.post(url),
            HttpMethod::Put => client.put(url),
            HttpMethod::Delete => client.delete(url),
            HttpMethod::Head => client.head(url),
        };
        if let Some(body) = body {
            request = request.body(body.clone());
        }
        for (key, value) in headers {
            if is_forbidden_request_header(key) {
                continue;
            }
            request = request.header(key, value);
        }
        if let Some(key) = credential {
            self.approve(plugin, state, ApprovalCategory::CredentialUse, key)
                .await?;
            let value = state.credentials.get(key).ok_or_else(|| {
                BrokerError::new(
                    BrokerErrorCode::NotFound,
                    format!("credential '{key}' not found"),
                )
            })?;
            request = request.header(reqwest::header::AUTHORIZATION, format!("Bearer {value}"));
        }
        let request = request
            .build()
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        Ok(FetchHop { client, request })
    }

    /// Streams a fetch response as broker frames (protocol v8 streaming).
    async fn fetch_stream(
        &self,
        plugin: &str,
        state: &PluginState,
        method: HttpMethod,
        url: &str,
        headers: &[(String, String)],
        credential: Option<&str>,
        body: Option<Vec<u8>>,
        max_bytes: u64,
        sink: &mut (dyn ene_plugin_proto::BrokerSink + Send),
    ) -> Result<(), BrokerError> {
        let mut current = url.to_string();
        for _hop in 0..=self.download.config.max_redirects {
            let hop = self
                .build_fetch_hop(
                    plugin,
                    state,
                    method,
                    &current,
                    headers,
                    credential,
                    body.as_ref(),
                )
                .await?;
            let response = hop
                .client
                .execute(hop.request)
                .await
                .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        BrokerError::new(
                            BrokerErrorCode::InvalidTarget,
                            "redirect without Location",
                        )
                    })?;
                current = url::Url::parse(&current)
                    .and_then(|base| base.join(location))
                    .map_err(|e| BrokerError::new(BrokerErrorCode::InvalidTarget, e.to_string()))?
                    .to_string();
                continue;
            }
            let response_headers = response
                .headers()
                .iter()
                .filter(|(key, _)| !is_forbidden_response_header(key.as_str()))
                .map(|(key, value)| {
                    (
                        key.as_str().to_string(),
                        value.to_str().unwrap_or("?").to_string(),
                    )
                })
                .collect();
            let status = response.status().as_u16();
            sink.write(&BrokerResponse::StreamStart {
                status,
                headers: response_headers,
            })
            .await
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
            let mut total = 0_u64;
            let mut stream = response.bytes_stream();
            use tokio_stream::StreamExt;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk
                    .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
                total = total.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
                if total > max_bytes {
                    return Err(BrokerError::new(
                        BrokerErrorCode::SizeExceeded,
                        format!("download exceeds {max_bytes} bytes"),
                    ));
                }
                sink.write(&BrokerResponse::StreamChunk {
                    data: chunk.to_vec(),
                })
                .await
                .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
            }
            sink.write(&BrokerResponse::StreamEnd)
                .await
                .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
            return Ok(());
        }
        Err(BrokerError::new(
            BrokerErrorCode::InvalidTarget,
            "too many redirects",
        ))
    }

    async fn fetch_loop(
        &self,
        plugin: &str,
        state: &PluginState,
        method: HttpMethod,
        url: &str,
        headers: &[(String, String)],
        credential: Option<&str>,
        body: Option<Vec<u8>>,
        max_bytes: u64,
        temp_path: Option<&Path>,
    ) -> Result<FetchBody, BrokerError> {
        let mut current = url.to_string();
        for _hop in 0..=self.download.config.max_redirects {
            let hop = self
                .build_fetch_hop(
                    plugin,
                    state,
                    method,
                    &current,
                    headers,
                    credential,
                    body.as_ref(),
                )
                .await?;
            let response = hop
                .client
                .execute(hop.request)
                .await
                .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        BrokerError::new(
                            BrokerErrorCode::InvalidTarget,
                            "redirect without Location",
                        )
                    })?;
                let next = url::Url::parse(&current)
                    .and_then(|base| base.join(location))
                    .map_err(|e| BrokerError::new(BrokerErrorCode::InvalidTarget, e.to_string()))?
                    .to_string();
                current = next;
                continue;
            }
            let final_url = response.url().to_string();
            let mime = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let response_headers = response
                .headers()
                .iter()
                .filter(|(key, _)| !is_forbidden_response_header(key.as_str()))
                .map(|(key, value)| {
                    (
                        key.as_str().to_string(),
                        value.to_str().unwrap_or("?").to_string(),
                    )
                })
                .collect();
            let status = response.status().as_u16();
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            use tokio_stream::StreamExt;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk
                    .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
                bytes.extend_from_slice(&chunk);
                if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
                    return Err(BrokerError::new(
                        BrokerErrorCode::SizeExceeded,
                        format!("download exceeds {max_bytes} bytes"),
                    ));
                }
            }
            let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            let sha256 = ene_artifact::sha256_hex(&bytes);
            if let Some(path) = temp_path {
                std::fs::write(path, &bytes)
                    .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
            }
            return Ok(FetchBody {
                status,
                headers: response_headers,
                bytes,
                final_url,
                size,
                sha256,
                mime,
            });
        }
        Err(BrokerError::new(
            BrokerErrorCode::InvalidTarget,
            "too many redirects",
        ))
    }

    fn origin_category(
        state: &PluginState,
        origin: &str,
        scheme: &str,
    ) -> Result<ApprovalCategory, BrokerError> {
        let manifest = state
            .manifest
            .as_ref()
            .ok_or_else(|| BrokerError::new(BrokerErrorCode::NotDeclared, "no manifest"))?;
        if manifest.fixed_origins.iter().any(|o| o.origin == origin) {
            return Ok(ApprovalCategory::FixedOriginNetwork);
        }
        if manifest.dynamic_web {
            return Ok(if scheme == "http" {
                ApprovalCategory::Http
            } else {
                ApprovalCategory::DynamicHttps
            });
        }
        Err(BrokerError::new(
            BrokerErrorCode::NotDeclared,
            format!("origin {origin} is not declared (no dynamic_web)"),
        ))
    }
}

struct FetchBody {
    status: u16,
    headers: Vec<(String, String)>,
    bytes: Vec<u8>,
    final_url: String,
    size: u64,
    sha256: String,
    mime: Option<String>,
}

/// One validated fetch hop ready to execute.
#[derive(Debug)]
struct FetchHop {
    client: reqwest::Client,
    request: reqwest::Request,
}

fn is_forbidden_request_header(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "authorization" | "cookie" | "proxy-authorization" | "host"
    )
}

fn is_forbidden_response_header(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "set-cookie" | "authorization" | "proxy-authenticate"
    )
}

/// SSRF guard: blocks loopback, private, link-local, cloud-metadata, and
/// reserved addresses, and re-checks every resolved address before connect
/// (DNS-rebinding protection).
pub struct SsrfPolicy {
    allow_loopback: bool,
}

impl SsrfPolicy {
    /// The production policy: every non-public address is blocked.
    #[must_use]
    pub fn production() -> Self {
        Self {
            allow_loopback: false,
        }
    }

    /// Resolves `host` and returns the addresses the policy allows.
    pub async fn resolve_allowed(&self, host: &str) -> Result<Vec<std::net::IpAddr>, String> {
        let addresses = tokio::net::lookup_host((host, 0))
            .await
            .map_err(|e| format!("DNS resolution failed: {e}"))?;
        let mut allowed = Vec::new();
        for address in addresses {
            let ip = address.ip();
            if self.is_allowed(ip) && !allowed.contains(&ip) {
                allowed.push(ip);
            }
        }
        Ok(allowed)
    }

    /// Whether the policy permits connecting to `ip`.
    #[must_use]
    pub fn is_allowed(&self, ip: std::net::IpAddr) -> bool {
        if ip.is_loopback() {
            return self.allow_loopback;
        }
        match ip {
            std::net::IpAddr::V4(v4) => !is_blocked_v4(v4),
            std::net::IpAddr::V6(v6) => !is_blocked_v6(v6),
        }
    }
}

fn is_blocked_v4(ip: std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    // 127.0.0.0/8 loopback (also reached via IPv4-mapped IPv6 addresses).
    if octets[0] == 127 {
        return true;
    }
    // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16 (private)
    if octets[0] == 10
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 168)
    {
        return true;
    }
    // 169.254.0.0/16 link-local (cloud metadata 169.254.169.254 included)
    if octets[0] == 169 && octets[1] == 254 {
        return true;
    }
    // 100.64.0.0/10 CGNAT, 192.0.0.0/24 IETF, 192.0.2.0/24 TEST-NET-1,
    // 198.18.0.0/15 benchmarking, 198.51.100.0/24 TEST-NET-2,
    // 203.0.113.0/24 TEST-NET-3, 240.0.0.0/4 reserved, 0.0.0.0/8,
    // 224.0.0.0/4 multicast
    let first = octets[0];
    if (100..=100).contains(&first) && (64..=127).contains(&octets[1])
        || first == 192 && octets[1] == 0
        || first == 198 && (18..=19).contains(&octets[1])
        || first >= 224
        || first == 0
    {
        return true;
    }
    false
}

fn is_blocked_v6(ip: std::net::Ipv6Addr) -> bool {
    let segments = ip.segments();
    // ::1 loopback, ::/128 unspecified, fe80::/10 link-local, fc00::/7 ULA,
    // ff00::/8 multicast, ::ffff:0:0/96 mapped IPv4 (check the mapped v4).
    if ip.is_loopback() || ip.is_unspecified() {
        return true;
    }
    // fc00::/7 ULA, fe80::/10 link-local, ff00::/8 multicast.
    if (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xff00) == 0xff00
    {
        return true;
    }
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_v4(v4);
    }
    false
}

// ── Process broker ──────────────────────────────────────────────────────

/// Commands that implicitly download and execute code; always denied.
const IMPLICIT_DOWNLOAD_PROGRAMS: &[&str] = &["npx", "uvx", "bunx"];

/// Package managers denied when invoked with install/yes flags.
const PACKAGE_MANAGERS: &[&str] = &["npm", "yarn", "pnpm", "pip", "pip3", "uv", "cargo"];

impl BrokerHub {
    async fn process_spawn(
        &self,
        plugin: &str,
        state: &PluginState,
        argv: Vec<String>,
        cwd: Option<String>,
        env: Vec<(String, String)>,
        timeout_ms: u64,
        max_output_bytes: u64,
    ) -> Result<BrokerResponse, BrokerError> {
        let Some(program) = argv.first() else {
            return Err(BrokerError::new(
                BrokerErrorCode::InvalidTarget,
                "empty argv",
            ));
        };
        let base = Path::new(program)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(program);
        let category = if is_shell(base) {
            ApprovalCategory::Shell
        } else {
            ApprovalCategory::ProcessSpawn
        };
        reject_implicit_download(base, &argv)?;
        self.approve(plugin, state, category, &argv.join(" "))
            .await?;
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]);
        command.kill_on_drop(true);
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        command.stdin(std::process::Stdio::null());
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        for (key, value) in env {
            if !is_forbidden_env(key.as_str()) {
                command.env(key, value);
            }
        }
        let timeout = if timeout_ms == 0 {
            std::time::Duration::from_mins(2)
        } else {
            std::time::Duration::from_millis(timeout_ms)
        };
        let mut child = command
            .spawn()
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        let pid = child.id().unwrap_or(0);
        state.processes.lock().insert(pid, argv.join(" "));
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let status = tokio::time::timeout(timeout, child.wait())
            .await
            .map_err(|_| BrokerError::denied("process timed out"))?
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        state.processes.lock().remove(&pid);
        let cap = usize::try_from(max_output_bytes.min(10 * 1024 * 1024)).unwrap_or(usize::MAX);
        let stdout_text = read_capped(stdout, cap).await;
        let stderr_text = read_capped_stderr(stderr, cap).await;
        Ok(BrokerResponse::ProcessSpawnOk {
            pid,
            exit_code: status.code(),
            stdout: stdout_text,
            stderr: stderr_text,
        })
    }

    fn process_signal(
        state: &PluginState,
        pid: u32,
        signal: u32,
    ) -> Result<BrokerResponse, BrokerError> {
        if !state.processes.lock().contains_key(&pid) {
            return Err(BrokerError::new(
                BrokerErrorCode::NotFound,
                "pid was not spawned through this broker",
            ));
        }
        signal_process(pid, signal)
    }
}

fn is_shell(program: &str) -> bool {
    matches!(
        program,
        "sh" | "bash" | "zsh" | "dash" | "cmd" | "powershell" | "pwsh"
    )
}

fn reject_implicit_download(program: &str, argv: &[String]) -> Result<(), BrokerError> {
    if IMPLICIT_DOWNLOAD_PROGRAMS.contains(&program) {
        return Err(BrokerError::denied(format!(
            "implicit-download runner '{program}' is banned"
        )));
    }
    if PACKAGE_MANAGERS.contains(&program) {
        let dangerous = argv.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "-y" | "--yes" | "install" | "add" | "i" | "update"
            )
        });
        if dangerous {
            return Err(BrokerError::denied(format!(
                "package-manager invocation '{program}' may download code"
            )));
        }
    }
    Ok(())
}

fn is_forbidden_env(key: &str) -> bool {
    matches!(
        key,
        "LD_PRELOAD"
            | "LD_LIBRARY_PATH"
            | "HOME"
            | "USERPROFILE"
            | "APPDATA"
            | "ENE_PLUGIN_SOCKET"
            | "ENE_BROKER_SOCKET"
            | "ENE_PLUGIN_TMPDIR"
    )
}

async fn read_capped(reader: Option<tokio::process::ChildStdout>, cap: usize) -> String {
    let Some(mut reader) = reader else {
        return String::new();
    };
    let mut buffer = Vec::new();
    let mut chunk = vec![0u8; 8192];
    while let Ok(n) = reader.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
        if buffer.len() >= cap {
            break;
        }
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

async fn read_capped_stderr(reader: Option<tokio::process::ChildStderr>, cap: usize) -> String {
    let Some(mut reader) = reader else {
        return String::new();
    };
    let mut buffer = Vec::new();
    let mut chunk = vec![0u8; 8192];
    while let Ok(n) = reader.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
        if buffer.len() >= cap {
            break;
        }
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

#[cfg(unix)]
fn signal_process(pid: u32, signal: u32) -> Result<BrokerResponse, BrokerError> {
    // SAFETY: kill with a caller-chosen signal against a pid the host itself
    // spawned for this plugin.
    let ret = unsafe { libc::kill(i32::try_from(pid).unwrap_or(-1), signal as i32) };
    if ret != 0 {
        return Err(BrokerError::new(
            BrokerErrorCode::NotFound,
            std::io::Error::last_os_error().to_string(),
        ));
    }
    Ok(BrokerResponse::ProcessSignalOk)
}

#[cfg(windows)]
fn signal_process(_pid: u32, _signal: u32) -> Result<BrokerResponse, BrokerError> {
    // TerminateProcess is not routed through the broker on Windows yet;
    // report unsupported rather than pretending.
    Err(BrokerError::new(
        BrokerErrorCode::Unavailable,
        "signal is not supported on Windows through the broker",
    ))
}

// ── Credential broker ───────────────────────────────────────────────────

impl BrokerHub {
    async fn credential_get(
        &self,
        plugin: &str,
        state: &PluginState,
        key: &str,
    ) -> Result<BrokerResponse, BrokerError> {
        self.approve(plugin, state, ApprovalCategory::CredentialUse, key)
            .await?;
        let value = state.credentials.get(key).ok_or_else(|| {
            BrokerError::new(
                BrokerErrorCode::NotFound,
                format!("credential '{key}' not found"),
            )
        })?;
        Ok(BrokerResponse::CredentialGetOk {
            value: value.clone(),
        })
    }

    async fn credential_list_keys(
        &self,
        plugin: &str,
        state: &PluginState,
    ) -> Result<BrokerResponse, BrokerError> {
        self.approve(plugin, state, ApprovalCategory::CredentialUse, "list_keys")
            .await?;
        let keys = state.credentials.keys().cloned().collect();
        Ok(BrokerResponse::CredentialListKeysOk { keys })
    }
}

// ── Artifact broker ─────────────────────────────────────────────────────

impl BrokerHub {
    fn artifact_services(&self) -> Result<&ArtifactServices, BrokerError> {
        self.artifact.as_ref().ok_or_else(|| {
            BrokerError::new(
                BrokerErrorCode::Unavailable,
                "artifact system is not configured",
            )
        })
    }

    async fn artifact_resolve(
        &self,
        plugin: &str,
        state: &PluginState,
        artifact_id: &str,
        version: Option<String>,
    ) -> Result<BrokerResponse, BrokerError> {
        let services = self.artifact_services()?;
        self.approve(
            plugin,
            state,
            ApprovalCategory::ModelInstall,
            &format!("resolve {artifact_id}"),
        )
        .await?;
        if let Some(installed) = services.installer.installed(artifact_id)
            && version.as_deref().is_none_or(|v| v == installed.version)
        {
            return Ok(BrokerResponse::ArtifactResolveOk {
                artifact: to_wire_artifact(&installed),
            });
        }
        let target = self
            .catalog_target(services, artifact_id, version.as_deref())
            .await?;
        Ok(BrokerResponse::ArtifactResolveOk {
            artifact: wire_artifact(artifact_id, &target),
        })
    }

    async fn artifact_install(
        &self,
        plugin: &str,
        state: &PluginState,
        artifact_id: &str,
        version: &str,
    ) -> Result<BrokerResponse, BrokerError> {
        let services = self.artifact_services()?;
        let target = self
            .catalog_target(services, artifact_id, Some(version))
            .await?;
        let installed = services.installer.installed(artifact_id);
        let category = match (&installed, target.kind) {
            (None, ArtifactKind::Plugin) => ApprovalCategory::PluginInstall,
            (None, ArtifactKind::Sidecar) => ApprovalCategory::SidecarInstall,
            (None, ArtifactKind::Model) => ApprovalCategory::ModelInstall,
            (Some(_), ArtifactKind::Plugin) => ApprovalCategory::PluginUpdate,
            (Some(_), ArtifactKind::Sidecar) => ApprovalCategory::SidecarUpdate,
            (Some(_), ArtifactKind::Model) => ApprovalCategory::ModelUpdate,
        };
        self.approve(plugin, state, category, &format!("{artifact_id} {version}"))
            .await?;
        let installed = services
            .installer
            .install(
                artifact_id,
                &target,
                &services.downloader,
                services.config.max_bytes,
                &|_| {
                    Err(ene_artifact::ArtifactError::RedirectRejected(
                        "artifact redirects are not followed".to_string(),
                    ))
                },
            )
            .await
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        Ok(BrokerResponse::ArtifactInstallOk {
            artifact: to_wire_artifact(&installed),
        })
    }

    fn artifact_rollback(&self, artifact_id: &str) -> Result<BrokerResponse, BrokerError> {
        let services = self.artifact_services()?;
        let rolled = services
            .installer
            .rollback(artifact_id)
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        Ok(BrokerResponse::ArtifactRollbackOk {
            artifact: to_wire_artifact(&rolled),
        })
    }

    async fn artifact_refresh(&self) -> Result<BrokerResponse, BrokerError> {
        let services = self.artifact_services()?;
        let metadata = self.fetch_catalog(services, true).await?;
        Ok(BrokerResponse::ArtifactRefreshOk {
            catalog_version: metadata.version,
        })
    }

    fn artifact_list(&self) -> Result<BrokerResponse, BrokerError> {
        let services = self.artifact_services()?;
        let state = services.installer.state();
        let mut artifacts: Vec<ArtifactInfo> =
            state.artifacts.values().map(to_wire_artifact).collect();
        artifacts.sort_by(|a, b| a.artifact_id.cmp(&b.artifact_id));
        Ok(BrokerResponse::ArtifactListOk { artifacts })
    }

    async fn catalog_target(
        &self,
        services: &ArtifactServices,
        artifact_id: &str,
        version: Option<&str>,
    ) -> Result<ArtifactTarget, BrokerError> {
        let metadata = self.catalog_metadata(services).await?;
        let target = metadata.artifacts.get(artifact_id).ok_or_else(|| {
            BrokerError::new(
                BrokerErrorCode::NotFound,
                format!("artifact '{artifact_id}' not in catalog"),
            )
        })?;
        if let Some(version) = version
            && target.version != version
        {
            return Err(BrokerError::new(
                BrokerErrorCode::NotFound,
                format!(
                    "artifact '{artifact_id}' version {version} not in catalog ({} available)",
                    target.version
                ),
            ));
        }
        Ok(target.clone())
    }

    async fn catalog_metadata(
        &self,
        services: &ArtifactServices,
    ) -> Result<ene_artifact::CatalogMetadata, BrokerError> {
        self.fetch_catalog(services, false).await
    }

    /// Fetches the signed catalog, using the cache until `refresh_hours`
    /// elapse. `force` bypasses the cache (manual refresh / installs).
    ///
    /// Every fetch re-verifies the signature, expiry, and rollback rules
    /// against the installed state, so a revoked or rolled-back artifact
    /// cannot stay installable past the refresh window.
    async fn fetch_catalog(
        &self,
        services: &ArtifactServices,
        force: bool,
    ) -> Result<ene_artifact::CatalogMetadata, BrokerError> {
        let now = now_ms();
        let refresh_ms = services
            .config
            .refresh_hours
            .max(1)
            .saturating_mul(3600 * 1000);
        if !force
            && let Some((catalog, fetched_at)) = services.catalog.lock().as_ref()
            && now.saturating_sub(*fetched_at) < refresh_ms
        {
            return Ok(catalog.clone());
        }
        let url = services.config.catalog_url.as_ref().ok_or_else(|| {
            BrokerError::new(BrokerErrorCode::Unavailable, "no catalog URL configured")
        })?;
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        if !response.status().is_success() {
            return Err(BrokerError::new(
                BrokerErrorCode::Internal,
                format!("catalog fetch failed: {}", response.status()),
            ));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        let signed: ene_artifact::SignedCatalog = serde_json::from_slice(&bytes)
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        let metadata = services
            .verifier
            .verify(&signed, &services.installer.installed_refs(), now)
            .map_err(|e| BrokerError::new(BrokerErrorCode::Denied, e.to_string()))?;
        *services.catalog.lock() = Some((metadata.clone(), now));
        Ok(metadata)
    }
}

fn to_wire_artifact(installed: &ene_artifact::InstalledArtifact) -> ArtifactInfo {
    ArtifactInfo {
        artifact_id: installed.id.clone(),
        version: installed.version.clone(),
        kind: match installed.kind {
            ArtifactKind::Plugin => WireArtifactKind::Plugin,
            ArtifactKind::Sidecar => WireArtifactKind::Sidecar,
            ArtifactKind::Model => WireArtifactKind::Model,
        },
        sha256: installed.sha256.clone(),
        size: installed.size,
    }
}

fn wire_artifact(artifact_id: &str, target: &ArtifactTarget) -> ArtifactInfo {
    ArtifactInfo {
        artifact_id: artifact_id.to_string(),
        version: target.version.clone(),
        kind: match target.kind {
            ArtifactKind::Plugin => WireArtifactKind::Plugin,
            ArtifactKind::Sidecar => WireArtifactKind::Sidecar,
            ArtifactKind::Model => WireArtifactKind::Model,
        },
        sha256: target.sha256.clone(),
        size: target.size,
    }
}

// ── Platform broker ─────────────────────────────────────────────────────

impl BrokerHub {
    async fn platform_open_external(
        &self,
        plugin: &str,
        state: &PluginState,
        url: &str,
    ) -> Result<BrokerResponse, BrokerError> {
        self.approve(plugin, state, ApprovalCategory::Platform, url)
            .await?;
        #[cfg(unix)]
        {
            let status = Command::new("xdg-open")
                .arg(url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await
                .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
            if !status.success() {
                return Err(BrokerError::new(
                    BrokerErrorCode::Internal,
                    "xdg-open failed",
                ));
            }
        }
        #[cfg(windows)]
        {
            let status = Command::new("cmd")
                .args(["/c", "start", "", url])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await
                .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
            if !status.success() {
                return Err(BrokerError::new(BrokerErrorCode::Internal, "start failed"));
            }
        }
        Ok(BrokerResponse::PlatformOpenExternalOk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_approval::ManifestPermission;

    #[test]
    fn ssrf_blocks_private_and_metadata_addresses() {
        let policy = SsrfPolicy::production();
        for ip in [
            "127.0.0.1",
            "127.0.0.2",
            "::1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "192.0.0.1",
            "198.18.0.1",
            "224.0.0.1",
            "0.0.0.0",
            "fe80::1",
            "fc00::1",
            "ff02::1",
            "::ffff:127.0.0.1",
        ] {
            let parsed: std::net::IpAddr = ip.parse().expect("ip");
            assert!(!policy.is_allowed(parsed), "{ip} must be blocked");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            let parsed: std::net::IpAddr = ip.parse().expect("ip");
            assert!(policy.is_allowed(parsed), "{ip} must be allowed");
        }
    }

    /// A `credential` key on a fetch request makes the host inject
    /// `Authorization: Bearer <value>` from its own state — the plugin only
    /// names the key. Uses a numeric public IP so no DNS/network is needed
    /// to build the hop.
    #[tokio::test]
    async fn credentialed_fetch_injects_bearer_from_host_state() {
        let mut policy = ApprovalPolicy::default();
        policy
            .categories
            .insert(ApprovalCategory::DynamicHttps, ApprovalMode::Allow);
        policy
            .categories
            .insert(ApprovalCategory::CredentialUse, ApprovalMode::Allow);
        let config = PluginConfig {
            enabled: true,
            approval: policy,
            list: HashMap::from([("web".to_string(), PluginEntry::default())]),
            ..PluginConfig::default()
        };
        let mut full = ene_config::EneConfig::default();
        full.set_section(&config).expect("set plugin section");
        let mut hub = BrokerHub::from_config(&full).expect("hub");
        let mut manifest = crate::manifest::builtin_manifest("web").expect("web manifest");
        manifest.permissions = vec![
            ManifestPermission {
                category: ApprovalCategory::DynamicHttps,
                max: ApprovalMode::Allow,
            },
            ManifestPermission {
                category: ApprovalCategory::CredentialUse,
                max: ApprovalMode::Allow,
            },
        ];
        let state = PluginState {
            manifest: Some(manifest),
            digest: None,
            fs_grants: Vec::new(),
            credentials: BTreeMap::from([("api_key".to_string(), "sk-test-123".to_string())]),
            processes: Mutex::new(HashMap::new()),
        };
        Arc::get_mut(&mut hub)
            .expect("sole hub owner")
            .plugins
            .insert("web".to_string(), state);

        let hop = hub
            .build_fetch_hop(
                "web",
                hub.plugins.get("web").expect("state"),
                HttpMethod::Get,
                "https://1.1.1.1/",
                &[],
                Some("api_key"),
                None,
            )
            .await
            .expect("hop");
        let authorization = hop
            .request
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .expect("authorization header");
        assert_eq!(authorization, "Bearer sk-test-123");
    }

    /// A named credential the host does not hold fails the request before
    /// any network work.
    #[tokio::test]
    async fn credentialed_fetch_rejects_unknown_key() {
        let mut policy = ApprovalPolicy::default();
        policy
            .categories
            .insert(ApprovalCategory::DynamicHttps, ApprovalMode::Allow);
        policy
            .categories
            .insert(ApprovalCategory::CredentialUse, ApprovalMode::Allow);
        let config = PluginConfig {
            enabled: true,
            approval: policy,
            list: HashMap::from([("web".to_string(), PluginEntry::default())]),
            ..PluginConfig::default()
        };
        let mut full = ene_config::EneConfig::default();
        full.set_section(&config).expect("set plugin section");
        let mut hub = BrokerHub::from_config(&full).expect("hub");
        let mut manifest = crate::manifest::builtin_manifest("web").expect("web manifest");
        manifest.permissions = vec![
            ManifestPermission {
                category: ApprovalCategory::DynamicHttps,
                max: ApprovalMode::Allow,
            },
            ManifestPermission {
                category: ApprovalCategory::CredentialUse,
                max: ApprovalMode::Allow,
            },
        ];
        let state = PluginState {
            manifest: Some(manifest),
            digest: None,
            fs_grants: Vec::new(),
            credentials: BTreeMap::new(),
            processes: Mutex::new(HashMap::new()),
        };
        Arc::get_mut(&mut hub)
            .expect("sole hub owner")
            .plugins
            .insert("web".to_string(), state);

        let err = hub
            .build_fetch_hop(
                "web",
                hub.plugins.get("web").expect("state"),
                HttpMethod::Get,
                "https://1.1.1.1/",
                &[],
                Some("api_key"),
                None,
            )
            .await
            .expect_err("missing credential must fail");
        assert!(
            err.message.contains("credential 'api_key' not found"),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn sanitize_file_name_strips_dangerous_characters() {
        assert_eq!(sanitize_file_name("report.pdf"), "report.pdf");
        assert_eq!(sanitize_file_name("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(sanitize_file_name("a<b>c:d"), "a_b_c_d");
        assert_eq!(sanitize_file_name(".."), "download");
        assert_eq!(sanitize_file_name(""), "download");
    }

    #[test]
    fn uniquify_appends_numeric_suffix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("file.txt");
        std::fs::write(&target, b"x").expect("write");
        let next = uniquify(&target);
        assert_eq!(next.file_name().unwrap().to_str(), Some("file-1.txt"));
        std::fs::write(&next, b"y").expect("write");
        assert_eq!(
            uniquify(&target).file_name().unwrap().to_str(),
            Some("file-2.txt")
        );
    }

    #[test]
    fn implicit_download_runners_are_rejected() {
        for program in IMPLICIT_DOWNLOAD_PROGRAMS {
            let argv = vec![(*program).to_string(), "-y".to_string(), "pkg".to_string()];
            assert!(reject_implicit_download(program, &argv).is_err());
        }
        assert!(reject_implicit_download("npm", &["npm".into(), "run".into()]).is_ok());
        assert!(reject_implicit_download("npm", &["npm".into(), "install".into()]).is_err());
        assert!(reject_implicit_download("ls", &["ls".into(), "-la".into()]).is_ok());
    }

    #[test]
    fn forbidden_headers_and_env_are_filtered() {
        assert!(is_forbidden_request_header("Authorization"));
        assert!(is_forbidden_request_header("cookie"));
        assert!(is_forbidden_request_header("Host"));
        assert!(!is_forbidden_request_header("X-Custom"));
        assert!(is_forbidden_response_header("set-cookie"));
        assert!(is_forbidden_env("LD_PRELOAD"));
        assert!(is_forbidden_env("HOME"));
        assert!(!is_forbidden_env("LANG"));
    }

    #[test]
    fn service_of_classifies_requests() {
        assert_eq!(
            service_of(&BrokerRequest::FileRead {
                path: "a".into(),
                max_bytes: None
            }),
            "file"
        );
        assert_eq!(
            service_of(&BrokerRequest::NetworkFetch {
                method: HttpMethod::Get,
                url: "https://x".into(),
                headers: vec![],
                credential: None,
                body: None,
                max_bytes: None,
            }),
            "network"
        );
        assert_eq!(
            service_of(&BrokerRequest::ProcessSpawn {
                argv: vec!["ls".into()],
                cwd: None,
                env: vec![],
                timeout_ms: 0,
                max_output_bytes: 1024,
            }),
            "process"
        );
        assert_eq!(service_of(&BrokerRequest::PlatformNow), "platform");
    }
}
