use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use ene_plugin_ipc::{
    BuiltinKind, EmbedRequest, EmbedResult, HostConn, LlmGenerateRequest, LlmGeneration,
    SttRequest, SttResult, ToolCall, TtsAudio, TtsRequest,
};
use ene_registry::{Layer, ToolDefinition, ToolInvoke, ToolRegistry, ToolSource, definitions_for};
use parking_lot::Mutex;
use serde_json::Value;
use thiserror::Error;
use tokio::time::timeout;
use uuid::Uuid;

use crate::broker::Broker;
use crate::fiber::{Effect, Fiber, FiberState, FiberUid};
use crate::profile::{
    ProfileApplyReport, active_provides, detect_require_cycles, inactive_cycle_fiber,
    missing_requires, waiting_fiber,
};
use crate::spawn::{SpawnOpts, SpawnedPlugin, discover_plugin_executable_in, spawn_plugin};

/// Profile row (manifest subset used at W1).
#[derive(Debug, Clone)]
pub struct ProfileRow {
    pub row_id: String,
    pub plugin: String,
    pub requires: Vec<String>,
    pub capabilities: Vec<String>,
    pub sandbox_required: bool,
    pub config: Value,
}

/// Circuit breaker threshold for consecutive spawn/call failures.
#[derive(Debug, Clone, Copy)]
pub struct CircuitBreakerConfig {
    pub max_failures: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self { max_failures: 3 }
    }
}

struct SupervisorInner {
    fibers: Mutex<HashMap<String, Fiber>>,
    profile_rows: Mutex<HashMap<String, ProfileRow>>,
    children: Mutex<HashMap<String, Child>>,
    sessions: Mutex<HashMap<String, Arc<PluginSession>>>,
    registry: Arc<ToolRegistry>,
    broker: Mutex<Broker>,
    workspace: PathBuf,
    circuit_breaker: CircuitBreakerConfig,
    failure_counts: Mutex<HashMap<String, u32>>,
    cycle_report: Mutex<Option<String>>,
    missing_requires: Mutex<HashMap<String, Vec<String>>>,
    prefer_in_process: AtomicBool,
    plugin_home: Mutex<PathBuf>,
    max_frame_bytes: AtomicU32,
    allow_unverified: AtomicBool,
}

/// Fiber supervisor. Reconcile is per-row; the core process is not restarted.
pub struct Supervisor {
    inner: Arc<SupervisorInner>,
}

struct PluginSession {
    conn: tokio::sync::Mutex<HostConn<tokio::net::UnixStream>>,
}

struct PluginInvoker {
    session: Arc<PluginSession>,
    row_id: String,
    plugin: String,
    inner: Arc<SupervisorInner>,
}

#[async_trait]
impl ToolInvoke for PluginInvoker {
    async fn invoke(&self, name: &str, args: Value) -> Result<Value, String> {
        let mut conn = self.session.conn.lock().await;
        let result = conn
            .call_tool(ToolCall {
                call_id: Uuid::now_v7().to_string(),
                tool_name: name.to_owned(),
                args,
                deadline_ms: None,
            })
            .await;
        match result {
            Ok(value) if value.status == "ok" => Ok(value.value),
            Ok(value) => {
                self.inner
                    .record_failure(&self.row_id, &self.plugin, self.inner.circuit_breaker);
                Err(value.value.to_string())
            }
            Err(err) => {
                self.inner
                    .record_failure(&self.row_id, &self.plugin, self.inner.circuit_breaker);
                Err(err.to_string())
            }
        }
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SupervisorError {
    #[error("sandbox required but unavailable")]
    SandboxRequired,
    #[error("unknown plugin {0}")]
    UnknownPlugin(String),
    #[error("provider unavailable: {0}")]
    ProviderUnavailable(String),
    #[error("spawn: {0}")]
    Spawn(String),
    #[error("circuit open for row {0}")]
    CircuitOpen(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Ipc(#[from] ene_plugin_ipc::IpcError),
}

impl SupervisorInner {
    fn record_failure(&self, row_id: &str, plugin: &str, config: CircuitBreakerConfig) {
        let mut counts = self.failure_counts.lock();
        let count = counts.entry(row_id.to_owned()).or_insert(0);
        *count += 1;
        if *count >= config.max_failures {
            drop(counts);
            self.trip_circuit(row_id, plugin);
        }
    }

    fn reset_failures(&self, row_id: &str) {
        self.failure_counts.lock().remove(row_id);
    }

    fn trip_circuit(&self, row_id: &str, plugin: &str) {
        let fiber = self
            .fibers
            .lock()
            .remove(row_id)
            .unwrap_or_else(|| Fiber::new(row_id, plugin));
        self.registry.unregister_source(&ToolSource::Plugin {
            plugin_id: fiber.plugin.clone(),
        });
        self.broker.lock().revoke_all(fiber.uid);
        if let Some(mut child) = self.children.lock().remove(row_id) {
            terminate_child(&mut child);
        }
        self.sessions.lock().remove(row_id);
        let mut failed = fiber;
        failed.state = FiberState::Failed;
        failed.dispose.clear();
        failed.wait_reason = Some("circuit open".to_owned());
        self.fibers.lock().insert(row_id.to_owned(), failed);
    }

    fn rollback_loading(&self, fiber: &Fiber) {
        self.registry.unregister_source(&ToolSource::Plugin {
            plugin_id: fiber.plugin.clone(),
        });
        self.broker.lock().revoke_all(fiber.uid);
        if let Some(mut child) = self.children.lock().remove(&fiber.row_id) {
            terminate_child(&mut child);
        }
        self.sessions.lock().remove(&fiber.row_id);
    }
}

impl Supervisor {
    #[must_use]
    pub fn new(workspace: PathBuf, registry: Arc<ToolRegistry>) -> Self {
        Self::with_config(workspace, registry, CircuitBreakerConfig::default())
    }

    #[must_use]
    pub fn with_config(
        workspace: PathBuf,
        registry: Arc<ToolRegistry>,
        circuit_breaker: CircuitBreakerConfig,
    ) -> Self {
        registry.set_workspace(workspace.clone());
        Self {
            inner: Arc::new(SupervisorInner {
                fibers: Mutex::new(HashMap::new()),
                profile_rows: Mutex::new(HashMap::new()),
                children: Mutex::new(HashMap::new()),
                sessions: Mutex::new(HashMap::new()),
                broker: Mutex::new(Broker::new(workspace.clone())),
                registry,
                workspace,
                circuit_breaker,
                failure_counts: Mutex::new(HashMap::new()),
                cycle_report: Mutex::new(None),
                missing_requires: Mutex::new(HashMap::new()),
                prefer_in_process: AtomicBool::new(false),
                plugin_home: Mutex::new(PathBuf::new()),
                max_frame_bytes: AtomicU32::new(1_048_576),
                allow_unverified: AtomicBool::new(false),
            }),
        }
    }

    /// Use in-process builtin handlers instead of spawning harness binaries.
    pub fn set_prefer_in_process_builtins(&self, yes: bool) {
        self.inner.prefer_in_process.store(yes, Ordering::Relaxed);
    }

    /// Install home, IPC frame cap, and digest policy from `plugins.*`.
    pub fn set_plugin_runtime(&self, home: PathBuf, max_frame_bytes: u32, allow_unverified: bool) {
        *self.inner.plugin_home.lock() = home;
        self.inner
            .max_frame_bytes
            .store(max_frame_bytes, Ordering::Relaxed);
        self.inner
            .allow_unverified
            .store(allow_unverified, Ordering::Relaxed);
    }

    #[must_use]
    pub fn discover(&self, plugin: &str) -> Option<PathBuf> {
        let home = self.inner.plugin_home.lock().clone();
        let home = (!home.as_os_str().is_empty()).then_some(home);
        discover_plugin_executable_in(plugin, home.as_deref())
    }

    #[must_use]
    pub fn circuit_breaker_config(&self) -> CircuitBreakerConfig {
        self.inner.circuit_breaker
    }

    #[must_use]
    pub fn registry(&self) -> Arc<ToolRegistry> {
        Arc::clone(&self.inner.registry)
    }

    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.inner.workspace
    }

    #[must_use]
    pub fn cycle_report(&self) -> Option<String> {
        self.inner.cycle_report.lock().clone()
    }

    #[must_use]
    pub fn missing_requires_for(&self, row_id: &str) -> Vec<String> {
        self.inner
            .missing_requires
            .lock()
            .get(row_id)
            .cloned()
            .unwrap_or_default()
    }

    #[must_use]
    pub fn failure_count(&self, row_id: &str) -> u32 {
        self.inner
            .failure_counts
            .lock()
            .get(row_id)
            .copied()
            .unwrap_or(0)
    }

    #[must_use]
    pub fn circuit_open(&self, row_id: &str) -> bool {
        self.inner
            .fibers
            .lock()
            .get(row_id)
            .is_some_and(|fiber| fiber.state == FiberState::Failed)
    }

    /// Reconcile the running fiber set to match `rows` without restarting unrelated rows.
    pub async fn apply_profile(&self, rows: &[ProfileRow]) -> ProfileApplyReport {
        let desired: HashSet<String> = rows.iter().map(|row| row.row_id.clone()).collect();
        let mut unloaded = Vec::new();
        let current: Vec<String> = self.inner.fibers.lock().keys().cloned().collect();
        for row_id in current {
            if !desired.contains(&row_id) {
                self.unload(&row_id).await;
                unloaded.push(row_id);
            }
        }

        let cycle_rows = detect_require_cycles(rows);
        if cycle_rows.is_empty() {
            self.inner.cycle_report.lock().take();
        } else {
            *self.inner.cycle_report.lock() = Some(format!(
                "circular requires among rows: {}",
                cycle_rows.join(", ")
            ));
            for row_id in &cycle_rows {
                let Some(row) = rows.iter().find(|row| row.row_id == *row_id) else {
                    continue;
                };
                self.inner
                    .fibers
                    .lock()
                    .insert(row_id.clone(), inactive_cycle_fiber(row));
            }
        }

        let mut activated = Vec::new();
        let mut waiting = Vec::new();
        let cycle_set: HashSet<&str> = cycle_rows.iter().map(String::as_str).collect();

        for row in rows {
            if cycle_set.contains(row.row_id.as_str()) {
                waiting.push(row.row_id.clone());
                continue;
            }
            if self.circuit_open(&row.row_id) {
                waiting.push(row.row_id.clone());
                continue;
            }
            if self
                .inner
                .fibers
                .lock()
                .get(&row.row_id)
                .is_some_and(|fiber| fiber.state == FiberState::Active)
            {
                continue;
            }
            let provides = active_provides(&self.inner.fibers.lock());
            let missing = missing_requires(row, &provides);
            if !missing.is_empty() {
                self.inner
                    .missing_requires
                    .lock()
                    .insert(row.row_id.clone(), missing);
                self.inner.fibers.lock().insert(
                    row.row_id.clone(),
                    waiting_fiber(row, "requires unsatisfied"),
                );
                waiting.push(row.row_id.clone());
                continue;
            }
            self.inner.missing_requires.lock().remove(&row.row_id);
            match self.try_activate_row(row).await {
                Ok(()) => activated.push(row.row_id.clone()),
                Err(_) => waiting.push(row.row_id.clone()),
            }
        }

        ProfileApplyReport {
            activated,
            unloaded,
            waiting,
            cycle_rows,
        }
    }

    async fn try_activate_row(&self, row: &ProfileRow) -> Result<(), SupervisorError> {
        if self.circuit_open(&row.row_id) {
            return Err(SupervisorError::CircuitOpen(row.row_id.clone()));
        }
        if self.inner.prefer_in_process.load(Ordering::Relaxed)
            && plugin_kind(&row.plugin).is_some()
        {
            let result = self.activate(row).map(|_| ());
            if result.is_err() {
                self.inner
                    .record_failure(&row.row_id, &row.plugin, self.inner.circuit_breaker);
            } else {
                self.inner.reset_failures(&row.row_id);
            }
            return result;
        }
        let result = if let Some(path) = self.discover(&row.plugin) {
            self.activate_process(row, &path).await.map(|_| ())
        } else if row.sandbox_required {
            Err(SupervisorError::UnknownPlugin(row.plugin.clone()))
        } else if plugin_kind(&row.plugin).is_some() {
            self.activate(row).map(|_| ())
        } else {
            Err(SupervisorError::UnknownPlugin(row.plugin.clone()))
        };
        if result.is_err() {
            self.inner
                .record_failure(&row.row_id, &row.plugin, self.inner.circuit_breaker);
        } else {
            self.inner.reset_failures(&row.row_id);
            self.inner
                .profile_rows
                .lock()
                .insert(row.row_id.clone(), row.clone());
        }
        result
    }

    /// Insert or reload a row in-process (test double). Production uses [`Self::activate_process`].
    pub fn activate(&self, row: &ProfileRow) -> Result<FiberUid, SupervisorError> {
        if row.sandbox_required && row_needs_os_sandbox(&row.plugin) && !ene_sandbox::supported() {
            return Err(SupervisorError::SandboxRequired);
        }
        let kind = plugin_kind(&row.plugin)
            .ok_or_else(|| SupervisorError::UnknownPlugin(row.plugin.clone()))?;
        let mut fiber = Fiber::new(&row.row_id, &row.plugin);
        fiber.requires.clone_from(&row.requires);
        fiber.sandbox_required = row.sandbox_required;
        fiber.state = FiberState::Loading;
        for def in definitions_for(kind) {
            fiber.push_effect(Effect::RegisterTool {
                name: def.name.clone(),
            });
            self.inner.registry.register(def);
        }
        {
            let mut broker = self.inner.broker.lock();
            for cap in &row.capabilities {
                broker.grant(fiber.uid, cap.clone());
                fiber.push_effect(Effect::BrokerGrant { op: cap.clone() });
            }
        }
        finish_active(&mut fiber);
        let uid = fiber.uid;
        self.inner.fibers.lock().insert(row.row_id.clone(), fiber);
        self.inner
            .profile_rows
            .lock()
            .insert(row.row_id.clone(), row.clone());
        Ok(uid)
    }

    /// Spawn the plugin binary or script, handshake, and register tools from `spec`.
    pub async fn activate_process(
        &self,
        row: &ProfileRow,
        binary: &Path,
    ) -> Result<FiberUid, SupervisorError> {
        if self.circuit_open(&row.row_id) {
            return Err(SupervisorError::CircuitOpen(row.row_id.clone()));
        }
        if row.sandbox_required && !ene_sandbox::supported() {
            return Err(SupervisorError::SandboxRequired);
        }
        let plugin_id = resolve_plugin_id(&row.plugin)
            .ok_or_else(|| SupervisorError::UnknownPlugin(row.plugin.clone()))?;
        let digest = crate::spawn::file_digest(binary)?;
        let mut fiber = Fiber::new(&row.row_id, &row.plugin);
        fiber.requires.clone_from(&row.requires);
        fiber.sandbox_required = row.sandbox_required;
        fiber.state = FiberState::Loading;
        let spawned = match spawn_plugin(SpawnOpts {
            binary,
            plugin_id: &plugin_id,
            digest: &digest,
            socket_dir: &self.inner.workspace.join("sockets"),
            row_id: &row.row_id,
            sandbox_required: row.sandbox_required,
            temp_dir: &self.inner.workspace.join("plugin-tmp").join(&row.row_id),
            workspace: &self.inner.workspace,
            config: &row.config,
            max_frame_bytes: self.inner.max_frame_bytes.load(Ordering::Relaxed),
            allow_unverified: self.inner.allow_unverified.load(Ordering::Relaxed),
        })
        .await
        {
            Ok(spawned) => spawned,
            Err(err) => {
                self.inner.rollback_loading(&fiber);
                self.inner
                    .record_failure(&row.row_id, &row.plugin, self.inner.circuit_breaker);
                return Err(err);
            }
        };
        if let Err(err) = self
            .apply_spawned(row, &plugin_id, &mut fiber, spawned)
            .await
        {
            self.inner.rollback_loading(&fiber);
            self.inner
                .record_failure(&row.row_id, &row.plugin, self.inner.circuit_breaker);
            return Err(err);
        }
        let uid = fiber.uid;
        self.inner.fibers.lock().insert(row.row_id.clone(), fiber);
        self.inner.reset_failures(&row.row_id);
        Ok(uid)
    }

    async fn apply_spawned(
        &self,
        row: &ProfileRow,
        plugin_id: &str,
        fiber: &mut Fiber,
        mut spawned: SpawnedPlugin,
    ) -> Result<(), SupervisorError> {
        let (child, mut conn) = spawned.take()?;
        let pid = child.id();
        fiber.push_effect(Effect::SpawnProcess { pid });
        self.inner.children.lock().insert(row.row_id.clone(), child);
        let faces = conn.negotiated().provider.clone();
        let tools = if conn.negotiated().tool.is_some() {
            timeout(Duration::from_secs(5), conn.list_tools())
                .await
                .map_err(|_| SupervisorError::Spawn("tool list timeout".to_owned()))??
        } else {
            Vec::new()
        };
        let session = Arc::new(PluginSession {
            conn: tokio::sync::Mutex::new(conn),
        });
        self.inner
            .sessions
            .lock()
            .insert(row.row_id.clone(), Arc::clone(&session));
        let invoke: Arc<dyn ToolInvoke> = Arc::new(PluginInvoker {
            session: Arc::clone(&session),
            row_id: row.row_id.clone(),
            plugin: row.plugin.clone(),
            inner: Arc::clone(&self.inner),
        });
        let source = ToolSource::Plugin {
            plugin_id: plugin_id.to_owned(),
        };
        if let Some(kind) = plugin_kind(&row.plugin) {
            for def in definitions_for(kind) {
                fiber.push_effect(Effect::RegisterTool {
                    name: def.name.clone(),
                });
                self.inner.registry.register_with(def, Arc::clone(&invoke));
            }
        } else if row.plugin == "tool.dummy" {
            let allowed: Vec<_> = tools
                .into_iter()
                .filter(|spec| spec.name == "dummy.ping" && spec.side_effects.is_empty())
                .collect();
            for spec in allowed {
                let def = ToolDefinition::from_wire(spec, source.clone());
                fiber.push_effect(Effect::RegisterTool {
                    name: def.name.clone(),
                });
                self.inner.registry.register_with(def, Arc::clone(&invoke));
            }
        } else if !row.plugin.starts_with("provider.") {
            for spec in tools {
                let def = ToolDefinition::from_wire(spec, source.clone());
                fiber.push_effect(Effect::RegisterTool {
                    name: def.name.clone(),
                });
                self.inner.registry.register_with(def, Arc::clone(&invoke));
            }
        }
        if let Some(faces) = faces {
            if faces.llm.is_some() {
                fiber.push_effect(Effect::BindSeam {
                    name: "llm".to_owned(),
                });
            }
            if faces.embed.is_some() {
                fiber.push_effect(Effect::BindSeam {
                    name: "embed".to_owned(),
                });
            }
            if faces.tts.is_some() {
                fiber.push_effect(Effect::BindSeam {
                    name: "tts".to_owned(),
                });
            }
            if faces.stt.is_some() {
                fiber.push_effect(Effect::BindSeam {
                    name: "stt".to_owned(),
                });
            }
        }
        {
            let mut broker = self.inner.broker.lock();
            for cap in &row.capabilities {
                broker.grant(fiber.uid, cap.clone());
                fiber.push_effect(Effect::BrokerGrant { op: cap.clone() });
            }
        }
        finish_active(fiber);
        Ok(())
    }

    /// Drain every fiber and kill leftover children. Used when HTTP stops.
    pub async fn shutdown(&self) {
        let ids: Vec<String> = self.inner.fibers.lock().keys().cloned().collect();
        for row_id in ids {
            self.unload(&row_id).await;
        }
        self.kill_all_children();
    }

    /// Kill remaining plugin processes without waiting for `DrainAck`.
    pub fn kill_all_children(&self) {
        for (_, mut child) in self.inner.children.lock().drain() {
            terminate_child(&mut child);
        }
        self.inner.sessions.lock().clear();
    }

    /// Unload a row: stop providing, then apply dispose LIFO (I-46).
    pub async fn unload(&self, row_id: &str) {
        let Some(mut fiber) = self.inner.fibers.lock().remove(row_id) else {
            return;
        };
        fiber.state = FiberState::Unloading;
        let session = self.inner.sessions.lock().remove(row_id);
        if let Some(session) = session {
            let drain = async { session.conn.lock().await.drain().await };
            drop(timeout(Duration::from_secs(2), drain).await);
        }
        if let Some(mut child) = self.inner.children.lock().remove(row_id) {
            terminate_child(&mut child);
        }
        self.inner.registry.unregister_source(&ToolSource::Plugin {
            plugin_id: fiber.plugin.clone(),
        });
        self.inner.broker.lock().revoke_all(fiber.uid);
        fiber.dispose.clear();
        fiber.state = FiberState::Inactive;
        self.inner.failure_counts.lock().remove(row_id);
        self.inner.missing_requires.lock().remove(row_id);
    }

    /// Disable one row; other rows keep uid and Active (I-49).
    pub async fn disable_row(&self, row_id: &str) {
        self.unload(row_id).await;
    }

    #[must_use]
    pub fn fiber(&self, row_id: &str) -> Option<Fiber> {
        self.inner.fibers.lock().get(row_id).cloned()
    }

    #[must_use]
    pub fn profile_row(&self, row_id: &str) -> Option<ProfileRow> {
        self.inner.profile_rows.lock().get(row_id).cloned()
    }

    #[must_use]
    pub fn active_row_ids(&self) -> Vec<String> {
        self.inner
            .fibers
            .lock()
            .iter()
            .filter(|(_, fiber)| fiber.state == FiberState::Active)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Snapshot of every profile row the supervisor currently tracks.
    #[must_use]
    pub fn list_fibers(&self) -> Vec<Fiber> {
        self.inner.fibers.lock().values().cloned().collect()
    }

    #[must_use]
    pub fn broker_has_grant(&self, uid: FiberUid, op: &str) -> bool {
        self.inner.broker.lock().has_grant(uid, op)
    }

    pub fn broker_fs_read(&self, uid: FiberUid, path: &Path) -> Result<String, crate::BrokerError> {
        self.inner.broker.lock().fs_read(uid, path)
    }

    #[must_use]
    pub fn surface_has_tool(&self, name: &str) -> bool {
        self.inner
            .registry
            .schemas(Layer::Surface)
            .iter()
            .any(|schema| schema.get("name").and_then(|v| v.as_str()) == Some(name))
    }

    /// First active plugin bound to `seam.llm` matching `plugin`, or any llm seam.
    pub async fn generate_llm(
        &self,
        plugin: &str,
        request: LlmGenerateRequest,
    ) -> Result<LlmGeneration, SupervisorError> {
        let session = self.provider_session(plugin, "seam.llm")?;
        let mut conn = session.conn.lock().await;
        conn.generate_llm(request).await.map_err(Into::into)
    }

    pub async fn embed(
        &self,
        plugin: &str,
        request: EmbedRequest,
    ) -> Result<EmbedResult, SupervisorError> {
        let session = self.provider_session(plugin, "seam.embed")?;
        let mut conn = session.conn.lock().await;
        conn.embed(request).await.map_err(Into::into)
    }

    pub async fn synthesize_tts(
        &self,
        plugin: &str,
        request: TtsRequest,
    ) -> Result<TtsAudio, SupervisorError> {
        let session = self.provider_session(plugin, "seam.tts")?;
        let mut conn = session.conn.lock().await;
        conn.synthesize_tts(request).await.map_err(Into::into)
    }

    pub async fn transcribe(
        &self,
        plugin: &str,
        request: SttRequest,
    ) -> Result<SttResult, SupervisorError> {
        let session = self.provider_session(plugin, "seam.stt")?;
        let mut conn = session.conn.lock().await;
        conn.transcribe(request).await.map_err(Into::into)
    }

    fn provider_session(
        &self,
        plugin: &str,
        seam: &str,
    ) -> Result<Arc<PluginSession>, SupervisorError> {
        let row_id = self
            .inner
            .fibers
            .lock()
            .values()
            .find(|fiber| {
                fiber.state == FiberState::Active
                    && fiber.plugin == plugin
                    && fiber.provides.iter().any(|key| key == seam)
            })
            .map(|fiber| fiber.row_id.clone())
            .ok_or_else(|| SupervisorError::ProviderUnavailable(plugin.to_owned()))?;
        self.inner
            .sessions
            .lock()
            .get(&row_id)
            .cloned()
            .ok_or_else(|| SupervisorError::ProviderUnavailable(plugin.to_owned()))
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.kill_all_children();
    }
}

fn resolve_plugin_id(plugin: &str) -> Option<String> {
    if let Some(kind) = plugin_kind(plugin) {
        return Some(kind.plugin_id().to_owned());
    }
    if plugin == "tool.dummy"
        || plugin.starts_with("tool.")
        || plugin.starts_with("provider.")
        || plugin.starts_with("mcp.")
    {
        return Some(plugin.to_owned());
    }
    None
}

/// BLAKE3 digest of a plugin script or binary (`blake3:<hex>`).
///
/// # Errors
///
/// Returns [`SupervisorError::Io`] when the file cannot be read.
pub fn manifest_digest(path: &Path) -> Result<String, SupervisorError> {
    crate::spawn::file_digest(path)
}

fn finish_active(fiber: &mut Fiber) {
    fiber.provides = fiber
        .dispose
        .iter()
        .filter_map(|effect| match effect {
            Effect::RegisterTool { name } => Some(format!("tool.{name}")),
            Effect::BrokerGrant { op } => Some(format!("broker.{op}")),
            Effect::BindSeam { name } => Some(format!("seam.{name}")),
            Effect::SpawnProcess { .. } => None,
        })
        .collect();
    fiber.state = FiberState::Active;
    fiber.wait_reason = None;
}

fn terminate_child(child: &mut Child) {
    if child.kill().is_err() {
        tracing::debug!("plugin child already gone");
    }
    drop(child.wait());
}

fn plugin_kind(plugin: &str) -> Option<BuiltinKind> {
    match plugin {
        "tool.fs" => Some(BuiltinKind::Fs),
        "tool.exec" => Some(BuiltinKind::Exec),
        "tool.web" => Some(BuiltinKind::Web),
        "tool.utility" => Some(BuiltinKind::Utility),
        "tool.app" => Some(BuiltinKind::App),
        _ => None,
    }
}

fn row_needs_os_sandbox(plugin: &str) -> bool {
    matches!(plugin, "tool.fs" | "tool.exec" | "tool.web")
}
