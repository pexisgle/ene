use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use ene_kernel::{HookEvent, LoopHooks, WaterfallGuard, WaterfallNext};
use ene_plugin_ipc::{
    BuiltinKind, EmbedRequest, EmbedResult, HostConn, InstallAssetRequest, InstallAssetResult,
    InstallPhase, InstallStatusRequest, InstallStatusResult, ListAssetsResult, ListModelsRequest,
    ListModelsResult, LlmChunk, LlmGenerateRequest, LlmGeneration, ProviderFaces,
    SetActiveAssetRequest, SetActiveAssetResult, SttRequest, SttResult, ToolBackgroundStart,
    ToolCall, TtsAudio, TtsRequest,
};
use ene_provider_assets::CatalogRegistry;
use ene_registry::{
    BuiltinExecutor, BuiltinInvoker, Layer, ToolDefinition, ToolInvoke, ToolRegistry, ToolSource,
    definitions_for, with_http_fetch,
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::time::timeout;
use uuid::Uuid;

use crate::assets_host::HostAssets;
use crate::broker::Broker;
use crate::fiber::{Effect, Fiber, FiberState, FiberUid};
use crate::profile::{
    ProfileApplyReport, active_provides, detect_require_cycles, inactive_cycle_fiber,
    missing_requires, waiting_fiber,
};
use crate::spawn::{SpawnOpts, SpawnedPlugin, discover_plugin_executable_in, spawn_plugin};

#[cfg(windows)]
type PluginStream = tokio::net::TcpStream;
#[cfg(unix)]
type PluginStream = tokio::net::UnixStream;

/// Profile row (manifest subset used at W1).
#[derive(Debug, Clone)]
pub struct ProfileRow {
    pub row_id: String,
    pub plugin: String,
    pub requires: Vec<String>,
    pub capabilities: Vec<String>,
    /// When non-empty, only these seams are bound from negotiated faces
    /// (`seam.llm`, `seam.embed`, …). Empty keeps every negotiated face.
    pub seams: Vec<String>,
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

struct AssetInstallJob {
    plugin: String,
    child: Child,
    conn: HostConn<PluginStream>,
}

struct SupervisorInner {
    fibers: Mutex<HashMap<String, Fiber>>,
    profile_rows: Mutex<HashMap<String, ProfileRow>>,
    children: Mutex<HashMap<String, Child>>,
    sessions: Mutex<HashMap<String, Arc<PluginSession>>>,
    asset_install_jobs: tokio::sync::Mutex<HashMap<String, AssetInstallJob>>,
    host_assets: Arc<HostAssets>,
    registry: Arc<ToolRegistry>,
    broker: Arc<Mutex<Broker>>,
    workspace: PathBuf,
    circuit_breaker: CircuitBreakerConfig,
    failure_counts: Mutex<HashMap<String, u32>>,
    cycle_report: Mutex<Option<String>>,
    missing_requires: Mutex<HashMap<String, Vec<String>>>,
    prefer_in_process: AtomicBool,
    plugin_home: Mutex<PathBuf>,
    max_frame_bytes: AtomicU32,
    allow_unverified: AtomicBool,
    loop_hooks: Mutex<Option<LoopHooks>>,
    waterfall_guards: Mutex<HashMap<String, Vec<WaterfallGuard<HookEvent>>>>,
    broker_servers: Mutex<HashMap<String, crate::broker_ipc::BrokerServer>>,
    #[cfg(test)]
    dispose_log: Mutex<Vec<Effect>>,
}

/// Fiber supervisor. Reconcile is per-row; the core process is not restarted.
pub struct Supervisor {
    inner: Arc<SupervisorInner>,
}

struct PluginSession {
    conn: tokio::sync::Mutex<HostConn<PluginStream>>,
}

struct PluginInvoker {
    session: Arc<PluginSession>,
    row_id: String,
    plugin: String,
    inner: Arc<SupervisorInner>,
}

struct HostWebInvoker {
    uid: FiberUid,
    inner: Arc<SupervisorInner>,
}

struct HostFsInvoker {
    uid: FiberUid,
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
            Ok(value) => Err(value.value.to_string()),
            Err(err) => {
                self.inner
                    .record_failure(&self.row_id, &self.plugin, self.inner.circuit_breaker);
                Err(err.to_string())
            }
        }
    }

    async fn start_background(
        &self,
        execution_id: &str,
        name: &str,
        args: Value,
        deadline_ms: Option<u64>,
    ) -> Result<(), String> {
        let mut conn = self.session.conn.lock().await;
        let started = conn
            .start_background(ToolBackgroundStart {
                call_id: Uuid::now_v7().to_string(),
                tool_name: name.to_owned(),
                args,
                execution_id: execution_id.to_owned(),
                deadline_ms,
            })
            .await
            .map_err(|err| err.to_string())?;
        if started.accepted {
            Ok(())
        } else {
            Err(started.error.unwrap_or_else(|| "not accepted".to_owned()))
        }
    }

    async fn cancel_background(&self, execution_id: &str) -> Result<String, String> {
        let mut conn = self.session.conn.lock().await;
        conn.cancel_background(execution_id)
            .await
            .map(|ack| ack.status)
            .map_err(|err| err.to_string())
    }

    async fn status_background(
        &self,
        execution_id: &str,
    ) -> Result<(String, Option<String>), String> {
        let mut conn = self.session.conn.lock().await;
        let status = conn
            .status_background(execution_id)
            .await
            .map_err(|err| err.to_string())?;
        Ok((status.phase, status.error_class))
    }

    async fn take_completion(
        &self,
        execution_id: &str,
    ) -> Option<ene_plugin_ipc::ToolExecutionComplete> {
        let mut conn = self.session.conn.lock().await;
        conn.take_completion(execution_id)
    }

    async fn take_completions(&self) -> Vec<ene_plugin_ipc::ToolExecutionComplete> {
        let mut conn = self.session.conn.lock().await;
        conn.take_completions()
    }
}

#[async_trait]
impl ToolInvoke for HostFsInvoker {
    async fn invoke(&self, name: &str, args: Value) -> Result<Value, String> {
        self.inner
            .broker
            .lock()
            .fs_invoke(self.uid, name, &args)
            .map_err(|err| err.to_string())
    }
}

#[async_trait]
impl ToolInvoke for HostWebInvoker {
    async fn invoke(&self, name: &str, args: Value) -> Result<Value, String> {
        if !self.inner.broker.lock().has_grant(self.uid, "net.fetch") {
            return Err("denied net.fetch".to_owned());
        }
        let uid = self.uid;
        let inner = Arc::clone(&self.inner);
        with_http_fetch(
            move |url| {
                inner
                    .broker
                    .lock()
                    .net_fetch(uid, url)
                    .map_err(|err| err.to_string())
            },
            || BuiltinExecutor.execute(name, &args),
        )
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
    #[error("loop hooks are not bound")]
    HooksNotBound,
    #[error("unknown fiber {0}")]
    UnknownFiber(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Ipc(#[from] ene_plugin_ipc::IpcError),
    #[error("broker ipc: {0}")]
    BrokerIpc(#[from] crate::BrokerIpcError),
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
        let mut fiber = self
            .fibers
            .lock()
            .remove(row_id)
            .unwrap_or_else(|| Fiber::new(row_id, plugin));
        self.dispose(&mut fiber);
        fiber.state = FiberState::Failed;
        fiber.wait_reason = Some("circuit open".to_owned());
        self.fibers.lock().insert(row_id.to_owned(), fiber);
    }

    fn rollback_loading(&self, fiber: &mut Fiber) {
        self.dispose(fiber);
    }

    fn dispose(&self, fiber: &mut Fiber) {
        #[cfg(test)]
        self.dispose_log.lock().clear();
        while let Some(effect) = fiber.dispose.pop() {
            #[cfg(test)]
            self.dispose_log.lock().push(effect.clone());
            self.invert(fiber, effect);
        }
        self.sweep_host_context(fiber);
    }

    fn invert(&self, fiber: &Fiber, effect: Effect) {
        match effect {
            Effect::ListenWaterfall { .. } => self.drop_one_waterfall(&fiber.row_id),
            Effect::BindSeam { .. } => {}
            Effect::BrokerListen { .. } => {
                if let Some(server) = self.broker_servers.lock().remove(&fiber.row_id) {
                    server.shutdown();
                }
            }
            Effect::SpawnProcess { .. } => {
                self.sessions.lock().remove(&fiber.row_id);
                if let Some(mut child) = self.children.lock().remove(&fiber.row_id) {
                    terminate_child(&mut child);
                }
            }
            Effect::BrokerGrant { op } => {
                self.broker.lock().revoke(fiber.uid, &op);
            }
            Effect::RegisterTool { name, owner } => {
                self.registry.unregister_owned(&name, &owner);
            }
        }
    }

    fn sweep_host_context(&self, fiber: &Fiber) {
        self.drop_waterfall(&fiber.row_id);
        if let Some(server) = self.broker_servers.lock().remove(&fiber.row_id) {
            server.shutdown();
        }
        self.sessions.lock().remove(&fiber.row_id);
        if let Some(mut child) = self.children.lock().remove(&fiber.row_id) {
            terminate_child(&mut child);
        }
        self.broker.lock().release_owned_sidecars(fiber.uid);
    }

    fn record_tool(&self, fiber: &mut Fiber, def: ToolDefinition, invoke: Arc<dyn ToolInvoke>) {
        let name = def.name.clone();
        let owner = fiber.uid.to_string();
        self.registry.register_owned(owner.clone(), def, invoke);
        fiber.push_effect(Effect::RegisterTool { name, owner });
    }

    fn record_grant(&self, fiber: &mut Fiber, op: String) {
        self.broker.lock().grant(fiber.uid, op.clone());
        fiber.push_effect(Effect::BrokerGrant { op });
    }

    fn record_spawn(&self, fiber: &mut Fiber, child: Child) {
        let pid = child.id();
        self.children.lock().insert(fiber.row_id.clone(), child);
        fiber.push_effect(Effect::SpawnProcess { pid });
    }

    fn drop_one_waterfall(&self, row_id: &str) {
        let mut guards = self.waterfall_guards.lock();
        let Some(stack) = guards.get_mut(row_id) else {
            return;
        };
        drop(stack.pop());
        if stack.is_empty() {
            guards.remove(row_id);
        }
    }

    fn drop_waterfall(&self, row_id: &str) {
        let Some(mut guards) = self.waterfall_guards.lock().remove(row_id) else {
            return;
        };
        while guards.pop().is_some() {}
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
        let catalog = Arc::new(CatalogRegistry::new());
        let host_assets = Arc::new(HostAssets::new(Arc::clone(&catalog)));
        Self {
            inner: Arc::new(SupervisorInner {
                fibers: Mutex::new(HashMap::new()),
                profile_rows: Mutex::new(HashMap::new()),
                children: Mutex::new(HashMap::new()),
                sessions: Mutex::new(HashMap::new()),
                asset_install_jobs: tokio::sync::Mutex::new(HashMap::new()),
                host_assets,
                broker: Arc::new(Mutex::new(Broker::new(workspace.clone()))),
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
                loop_hooks: Mutex::new(None),
                waterfall_guards: Mutex::new(HashMap::new()),
                broker_servers: Mutex::new(HashMap::new()),
                #[cfg(test)]
                dispose_log: Mutex::new(Vec::new()),
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

    /// Share the dialogue-lane waterfall so a fiber can subscribe.
    pub fn set_loop_hooks(&self, hooks: LoopHooks) {
        *self.inner.loop_hooks.lock() = Some(hooks);
    }

    /// Register `agent/pre-step`. The guard is dropped LIFO on unload.
    pub fn listen_pre_step<F>(&self, row_id: &str, listener: F) -> Result<(), SupervisorError>
    where
        F: Fn(HookEvent, WaterfallNext<HookEvent>) -> HookEvent + Send + Sync + 'static,
    {
        self.listen_waterfall(
            row_id,
            "agent/pre-step",
            |hooks, listener| hooks.pre_step.listen(listener),
            listener,
        )
    }

    /// Register `agent/request`. The guard is dropped LIFO on unload.
    pub fn listen_request<F>(&self, row_id: &str, listener: F) -> Result<(), SupervisorError>
    where
        F: Fn(HookEvent, WaterfallNext<HookEvent>) -> HookEvent + Send + Sync + 'static,
    {
        self.listen_waterfall(
            row_id,
            "agent/request",
            |hooks, listener| hooks.request.listen(listener),
            listener,
        )
    }

    fn listen_waterfall<F>(
        &self,
        row_id: &str,
        point: &str,
        subscribe: impl FnOnce(&LoopHooks, F) -> WaterfallGuard<HookEvent>,
        listener: F,
    ) -> Result<(), SupervisorError>
    where
        F: Fn(HookEvent, WaterfallNext<HookEvent>) -> HookEvent + Send + Sync + 'static,
    {
        let Some(hooks) = self.inner.loop_hooks.lock().clone() else {
            return Err(SupervisorError::HooksNotBound);
        };
        {
            let fibers = self.inner.fibers.lock();
            if !fibers.contains_key(row_id) {
                return Err(SupervisorError::UnknownFiber(row_id.to_owned()));
            }
        }
        let guard = subscribe(&hooks, listener);
        {
            let mut fibers = self.inner.fibers.lock();
            let Some(fiber) = fibers.get_mut(row_id) else {
                drop(guard);
                return Err(SupervisorError::UnknownFiber(row_id.to_owned()));
            };
            fiber.push_effect(Effect::ListenWaterfall {
                point: point.to_owned(),
            });
            self.inner
                .waterfall_guards
                .lock()
                .entry(row_id.to_owned())
                .or_default()
                .push(guard);
        }
        Ok(())
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
                Err(err) => {
                    self.inner
                        .profile_rows
                        .lock()
                        .insert(row.row_id.clone(), row.clone());
                    if !self.circuit_open(&row.row_id) {
                        self.inner
                            .fibers
                            .lock()
                            .insert(row.row_id.clone(), waiting_fiber(row, err.to_string()));
                    }
                    waiting.push(row.row_id.clone());
                }
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
        let host_invoke: Option<Arc<dyn ToolInvoke>> = match row.plugin.as_str() {
            "tool.web" => Some(Arc::new(HostWebInvoker {
                uid: fiber.uid,
                inner: Arc::clone(&self.inner),
            })),
            "tool.fs" => Some(Arc::new(HostFsInvoker {
                uid: fiber.uid,
                inner: Arc::clone(&self.inner),
            })),
            _ => None,
        };
        let builtin_invoke = Arc::new(BuiltinInvoker) as Arc<dyn ToolInvoke>;
        for def in definitions_for(kind) {
            let invoke = host_invoke
                .as_ref()
                .map_or_else(|| Arc::clone(&builtin_invoke), Arc::clone);
            self.inner.record_tool(&mut fiber, def, invoke);
        }
        for cap in &row.capabilities {
            self.inner.record_grant(&mut fiber, cap.clone());
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
        let mut config = row.config.clone();
        {
            let mut broker = self.inner.broker.lock();
            config = crate::managed_sidecar::inject_managed_sidecar(
                &row.plugin,
                &config,
                fiber.uid,
                &mut broker,
            )?;
        }
        let spawn_token = Uuid::now_v7().to_string();
        let broker_server = if plugin_kind(&row.plugin).is_some() {
            Some(crate::broker_ipc::BrokerServer::bind(
                Arc::clone(&self.inner.broker),
                fiber.uid,
                &row.row_id,
                &spawn_token,
            )?)
        } else {
            None
        };
        let broker_socket = broker_server
            .as_ref()
            .map(|server| server.endpoint().to_owned());
        let spawned = match spawn_plugin(SpawnOpts {
            binary,
            plugin_id: &plugin_id,
            digest: &digest,
            row_id: &row.row_id,
            sandbox_required: row.sandbox_required,
            temp_dir: &self.inner.workspace.join("plugin-tmp").join(&row.row_id),
            workspace: &self.inner.workspace,
            config: &config,
            broker_socket: broker_socket.as_deref(),
            broker_token: Some(&spawn_token),
            max_frame_bytes: self.inner.max_frame_bytes.load(Ordering::Relaxed),
            allow_unverified: self.inner.allow_unverified.load(Ordering::Relaxed),
        })
        .await
        {
            Ok(spawned) => spawned,
            Err(err) => {
                self.inner.rollback_loading(&mut fiber);
                self.inner
                    .record_failure(&row.row_id, &row.plugin, self.inner.circuit_breaker);
                if let Some(server) = broker_server {
                    server.shutdown();
                }
                return Err(err);
            }
        };
        if let Err(err) = self
            .apply_spawned(row, &plugin_id, &mut fiber, spawned)
            .await
        {
            self.inner.rollback_loading(&mut fiber);
            self.inner
                .record_failure(&row.row_id, &row.plugin, self.inner.circuit_breaker);
            if let Some(server) = broker_server {
                server.shutdown();
            }
            return Err(err);
        }
        let uid = fiber.uid;
        if let Some(server) = broker_server {
            fiber.broker_socket.clone_from(&broker_socket);
            fiber.push_effect(Effect::BrokerListen {
                path: fiber.broker_socket.clone().unwrap_or_default(),
            });
            self.inner
                .broker_servers
                .lock()
                .insert(row.row_id.clone(), server);
        }
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
        self.inner.record_spawn(fiber, child);
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
        let invoke: Arc<dyn ToolInvoke> = if row.plugin == "tool.web" {
            Arc::new(HostWebInvoker {
                uid: fiber.uid,
                inner: Arc::clone(&self.inner),
            })
        } else if row.plugin == "tool.fs" {
            Arc::new(HostFsInvoker {
                uid: fiber.uid,
                inner: Arc::clone(&self.inner),
            })
        } else {
            Arc::new(PluginInvoker {
                session: Arc::clone(&session),
                row_id: row.row_id.clone(),
                plugin: row.plugin.clone(),
                inner: Arc::clone(&self.inner),
            })
        };
        let source = ToolSource::Plugin {
            plugin_id: plugin_id.to_owned(),
        };
        if let Some(kind) = plugin_kind(&row.plugin) {
            let advertised: HashSet<String> = tools.iter().map(|spec| spec.name.clone()).collect();
            for mut def in definitions_for(kind) {
                if !advertised.contains(&def.name) {
                    continue;
                }
                def.source = source.clone();
                self.inner.record_tool(fiber, def, Arc::clone(&invoke));
            }
        } else if row.plugin == "tool.dummy" {
            let allowed: Vec<_> = tools
                .into_iter()
                .filter(|spec| spec.name == "dummy.ping" && spec.side_effects.is_empty())
                .collect();
            for spec in allowed {
                let def = ToolDefinition::from_wire(spec, source.clone());
                self.inner.record_tool(fiber, def, Arc::clone(&invoke));
            }
        } else if !row.plugin.starts_with("provider.") {
            for spec in tools {
                let def = ToolDefinition::from_wire(spec, source.clone());
                self.inner.record_tool(fiber, def, Arc::clone(&invoke));
            }
        }
        if let Some(faces) = faces {
            bind_negotiated_seams(fiber, &faces, &row.seams);
        }
        for cap in &row.capabilities {
            self.inner.record_grant(fiber, cap.clone());
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

    /// Unload a row: stop providing, drain the plugin session, then apply dispose LIFO.
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
        self.inner.dispose(&mut fiber);
        self.inner.failure_counts.lock().remove(row_id);
        self.inner.missing_requires.lock().remove(row_id);
    }

    #[cfg(test)]
    pub(crate) fn last_dispose(&self) -> Vec<Effect> {
        self.inner.dispose_log.lock().clone()
    }

    #[cfg(test)]
    pub(crate) fn rollback_active(&self, row_id: &str) {
        let Some(mut fiber) = self.inner.fibers.lock().remove(row_id) else {
            return;
        };
        self.inner.rollback_loading(&mut fiber);
    }

    #[cfg(test)]
    pub(crate) fn push_effect(&self, row_id: &str, effect: Effect) {
        if let Some(fiber) = self.inner.fibers.lock().get_mut(row_id) {
            fiber.push_effect(effect);
        }
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

    /// Active fiber `row_id` for a provider task (`ai.tasks.chat`, …).
    pub async fn generate_llm(
        &self,
        row_id: &str,
        request: LlmGenerateRequest,
    ) -> Result<LlmGeneration, SupervisorError> {
        self.generate_llm_streaming(row_id, request, |_| {}).await
    }

    /// LLM generate that forwards matching `LlmChunk` frames to `on_chunk`.
    pub async fn generate_llm_streaming<F>(
        &self,
        row_id: &str,
        request: LlmGenerateRequest,
        on_chunk: F,
    ) -> Result<LlmGeneration, SupervisorError>
    where
        F: FnMut(LlmChunk),
    {
        let session = self.session_by_row(row_id)?;
        let mut conn = session.conn.lock().await;
        conn.generate_llm_streaming(request, on_chunk)
            .await
            .map_err(Into::into)
    }

    pub async fn embed(
        &self,
        row_id: &str,
        request: EmbedRequest,
    ) -> Result<EmbedResult, SupervisorError> {
        let session = self.session_by_row(row_id)?;
        let mut conn = session.conn.lock().await;
        conn.embed(request).await.map_err(Into::into)
    }

    pub async fn synthesize_tts(
        &self,
        row_id: &str,
        request: TtsRequest,
    ) -> Result<TtsAudio, SupervisorError> {
        let session = self.session_by_row(row_id)?;
        let mut conn = session.conn.lock().await;
        conn.synthesize_tts(request).await.map_err(Into::into)
    }

    pub async fn transcribe(
        &self,
        row_id: &str,
        request: SttRequest,
    ) -> Result<SttResult, SupervisorError> {
        let session = self.session_by_row(row_id)?;
        let mut conn = session.conn.lock().await;
        conn.transcribe(request).await.map_err(Into::into)
    }

    /// Plugin settings schema, with secret keys filled from schema metadata.
    ///
    /// # Errors
    ///
    /// Returns [`SupervisorError`] when the fiber is missing or IPC fails.
    pub async fn plugin_config_schema(
        &self,
        row_id: &str,
    ) -> Result<ene_plugin_ipc::PluginConfigSchema, SupervisorError> {
        let Ok(session) = self.session_by_row(row_id) else {
            return Ok(ene_plugin_ipc::PluginConfigSchema::default());
        };
        let mut conn = session.conn.lock().await;
        let mut schema = conn.config_schema().await?;
        schema.schema = ene_plugin_ipc::scrub_schema_secrets(&schema.schema);
        if schema.secret_keys.is_empty() {
            schema.secret_keys = ene_plugin_ipc::secret_keys_from_schema(&schema.schema);
        }
        Ok(schema)
    }

    /// Validate candidate settings. Does not mutate the last-good config.
    ///
    /// # Errors
    ///
    /// Returns [`SupervisorError`] when the fiber is missing or IPC fails.
    pub async fn plugin_config_validate(
        &self,
        row_id: &str,
        values: Value,
    ) -> Result<ene_plugin_ipc::PluginConfigValidateResult, SupervisorError> {
        let session = self.session_by_row(row_id)?;
        let mut conn = session.conn.lock().await;
        conn.config_validate(values).await.map_err(Into::into)
    }

    /// Dynamic options for one field. Enumeration failure degrades to fallback.
    ///
    /// # Errors
    ///
    /// Returns [`SupervisorError`] when the fiber is missing or IPC fails.
    pub async fn plugin_config_options(
        &self,
        row_id: &str,
        field: &str,
    ) -> Result<ene_plugin_ipc::PluginConfigOptionsResult, SupervisorError> {
        let session = self.session_by_row(row_id)?;
        let mut conn = session.conn.lock().await;
        match conn.config_options(field).await {
            Ok(result) => Ok(result),
            Err(_) => Ok(ene_plugin_ipc::PluginConfigOptionsResult::unsupported()),
        }
    }

    /// Apply settings after validate. On failure the previous ProfileRow.config stays.
    ///
    /// # Errors
    ///
    /// Returns [`SupervisorError`] when the fiber is missing or IPC fails.
    pub async fn plugin_config_apply(
        &self,
        row_id: &str,
        values: Value,
    ) -> Result<ene_plugin_ipc::PluginConfigApplyResult, SupervisorError> {
        let previous = self
            .inner
            .profile_rows
            .lock()
            .get(row_id)
            .map(|row| row.config.clone())
            .unwrap_or_default();
        let session = self.session_by_row(row_id)?;
        let mut conn = session.conn.lock().await;
        let result = conn.config_apply(values.clone()).await?;
        drop(conn);
        commit_applied_config(
            &self.inner.profile_rows,
            row_id,
            values,
            result.ok,
            previous,
        );
        Ok(result)
    }

    /// Redacted current settings for API/UI. Secrets never leave this method.
    #[must_use]
    pub fn plugin_config_values(&self, row_id: &str, schema: &Value) -> Value {
        let values = self
            .inner
            .profile_rows
            .lock()
            .get(row_id)
            .map_or_else(|| json!({}), |row| row.config.clone());
        ene_plugin_ipc::redact_config_values(schema, &values)
    }

    /// Vendor model ids for a provider plugin.
    ///
    /// Always a one-shot spawn with `base_url` only. Listing on a live
    /// generation fiber sends a new RPC that older plugin processes cannot
    /// decode, which closes the socket and takes down chat.
    pub async fn list_models(
        &self,
        plugin: &str,
        request: ListModelsRequest,
    ) -> Result<ListModelsResult, SupervisorError> {
        self.probe_list_models(plugin, request).await
    }

    pub async fn list_assets(&self, plugin: &str) -> Result<ListAssetsResult, SupervisorError> {
        self.inner
            .host_assets
            .list_assets(plugin, self.probe_list_assets(plugin))
            .await
    }

    pub async fn refresh_asset_catalog(
        &self,
        plugin: &str,
    ) -> Result<ene_provider_assets::RuntimeCatalog, SupervisorError> {
        self.inner.host_assets.refresh_catalog(plugin).await
    }

    pub async fn install_asset(
        &self,
        plugin: &str,
        request: InstallAssetRequest,
    ) -> Result<InstallAssetResult, SupervisorError> {
        self.inner
            .host_assets
            .install_asset(
                plugin,
                request.clone(),
                self.probe_install_asset(plugin, request),
            )
            .await
    }

    async fn probe_install_asset(
        &self,
        plugin: &str,
        request: InstallAssetRequest,
    ) -> Result<InstallAssetResult, SupervisorError> {
        let (mut child, mut conn) = self.spawn_probe(plugin, &json!({})).await?;
        let listed = timeout(Duration::from_secs(30), conn.install_asset(request)).await;
        let result = match listed {
            Ok(Ok(result)) => result,
            Ok(Err(err)) => {
                drop(timeout(Duration::from_secs(2), conn.drain()).await);
                terminate_child(&mut child);
                return Err(err.into());
            }
            Err(_) => {
                drop(timeout(Duration::from_secs(2), conn.drain()).await);
                terminate_child(&mut child);
                return Err(SupervisorError::Spawn("provider probe timeout".to_owned()));
            }
        };
        if result.job_id.is_empty() || result.error.is_some() {
            drop(timeout(Duration::from_secs(2), conn.drain()).await);
            terminate_child(&mut child);
            return Ok(result);
        }
        self.inner.asset_install_jobs.lock().await.insert(
            result.job_id.clone(),
            AssetInstallJob {
                plugin: plugin.to_owned(),
                child,
                conn,
            },
        );
        Ok(result)
    }

    pub async fn install_asset_status(
        &self,
        plugin: &str,
        request: InstallStatusRequest,
    ) -> Result<InstallStatusResult, SupervisorError> {
        if let Ok(status) = self.inner.host_assets.install_status(request.clone()).await
            && status.error.as_deref() != Some("job not found")
        {
            return Ok(status);
        }
        let job_id = request.job_id.clone();
        let mut jobs = self.inner.asset_install_jobs.lock().await;
        let Some(job) = jobs.get_mut(&job_id) else {
            return Ok(InstallStatusResult {
                error: Some("job not found".to_owned()),
                ..InstallStatusResult::default()
            });
        };
        if job.plugin != plugin {
            return Ok(InstallStatusResult {
                error: Some("job not found".to_owned()),
                ..InstallStatusResult::default()
            });
        }
        let listed = timeout(Duration::from_secs(30), job.conn.install_status(request)).await;
        let status = match listed {
            Ok(Ok(status)) => status,
            Ok(Err(err)) => {
                if let Some(mut job) = jobs.remove(&job_id) {
                    finish_asset_install_job(&mut job).await;
                }
                return Err(err.into());
            }
            Err(_) => {
                if let Some(mut job) = jobs.remove(&job_id) {
                    finish_asset_install_job(&mut job).await;
                }
                return Err(SupervisorError::Spawn("provider probe timeout".to_owned()));
            }
        };
        if matches!(
            status.phase,
            Some(InstallPhase::Done | InstallPhase::Failed)
        ) && let Some(mut job) = jobs.remove(&job_id)
        {
            finish_asset_install_job(&mut job).await;
        }
        Ok(status)
    }

    pub async fn set_active_asset(
        &self,
        plugin: &str,
        request: SetActiveAssetRequest,
    ) -> Result<SetActiveAssetResult, SupervisorError> {
        self.inner
            .host_assets
            .set_active_asset(
                plugin,
                request.clone(),
                self.probe_set_active_asset(plugin, request),
            )
            .await
    }

    async fn probe_set_active_asset(
        &self,
        plugin: &str,
        request: SetActiveAssetRequest,
    ) -> Result<SetActiveAssetResult, SupervisorError> {
        let (mut child, mut conn) = self.spawn_probe(plugin, &json!({})).await?;
        let listed = timeout(Duration::from_secs(30), conn.set_active_asset(request)).await;
        self.finish_probe(&mut child, &mut conn, listed).await
    }

    async fn probe_list_assets(&self, plugin: &str) -> Result<ListAssetsResult, SupervisorError> {
        let (mut child, mut conn) = self.spawn_probe(plugin, &json!({})).await?;
        let listed = timeout(Duration::from_secs(30), conn.list_assets()).await;
        self.finish_probe(&mut child, &mut conn, listed).await
    }

    async fn spawn_probe(
        &self,
        plugin: &str,
        config: &Value,
    ) -> Result<(Child, HostConn<PluginStream>), SupervisorError> {
        let plugin_id = resolve_plugin_id(plugin)
            .ok_or_else(|| SupervisorError::UnknownPlugin(plugin.to_owned()))?;
        let binary = self
            .discover(plugin)
            .ok_or_else(|| SupervisorError::Spawn(format!("provider binary missing: {plugin}")))?;
        let digest = crate::spawn::file_digest(&binary)?;
        let row_id = format!("probe-{}", Uuid::now_v7());
        let mut spawned = spawn_plugin(SpawnOpts {
            binary: &binary,
            plugin_id: &plugin_id,
            digest: &digest,
            row_id: &row_id,
            sandbox_required: false,
            temp_dir: &self.inner.workspace.join("plugin-tmp").join(&row_id),
            workspace: &self.inner.workspace,
            config,
            broker_socket: None,
            broker_token: None,
            max_frame_bytes: self.inner.max_frame_bytes.load(Ordering::Relaxed),
            allow_unverified: self.inner.allow_unverified.load(Ordering::Relaxed),
        })
        .await?;
        spawned.take()
    }

    async fn finish_probe<T>(
        &self,
        child: &mut Child,
        conn: &mut HostConn<PluginStream>,
        listed: Result<Result<T, ene_plugin_ipc::IpcError>, tokio::time::error::Elapsed>,
    ) -> Result<T, SupervisorError> {
        drop(timeout(Duration::from_secs(2), conn.drain()).await);
        terminate_child(child);
        match listed {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(err)) => Err(err.into()),
            Err(_) => Err(SupervisorError::Spawn("provider probe timeout".to_owned())),
        }
    }

    async fn probe_list_models(
        &self,
        plugin: &str,
        request: ListModelsRequest,
    ) -> Result<ListModelsResult, SupervisorError> {
        let config = json!({ "base_url": request.base_url });
        let (mut child, mut conn) = self.spawn_probe(plugin, &config).await?;
        let listed = timeout(Duration::from_secs(30), conn.list_models(request)).await;
        self.finish_probe(&mut child, &mut conn, listed).await
    }

    fn session_by_row(&self, row_id: &str) -> Result<Arc<PluginSession>, SupervisorError> {
        let active = self
            .inner
            .fibers
            .lock()
            .get(row_id)
            .is_some_and(|fiber| fiber.state == FiberState::Active);
        if !active {
            return Err(SupervisorError::ProviderUnavailable(row_id.to_owned()));
        }
        self.inner
            .sessions
            .lock()
            .get(row_id)
            .cloned()
            .ok_or_else(|| SupervisorError::ProviderUnavailable(row_id.to_owned()))
    }
}

pub(crate) fn commit_applied_config(
    rows: &Mutex<HashMap<String, ProfileRow>>,
    row_id: &str,
    values: Value,
    ok: bool,
    previous: Value,
) {
    if let Some(row) = rows.lock().get_mut(row_id) {
        row.config = if ok { values } else { previous };
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
            Effect::RegisterTool { name, .. } => Some(format!("tool.{name}")),
            Effect::BrokerGrant { op } => Some(format!("broker.{op}")),
            Effect::BindSeam { name } => Some(format!("seam.{name}")),
            Effect::BrokerListen { .. }
            | Effect::SpawnProcess { .. }
            | Effect::ListenWaterfall { .. } => None,
        })
        .collect();
    fiber.state = FiberState::Active;
    fiber.wait_reason = None;
}

fn bind_negotiated_seams(fiber: &mut Fiber, faces: &ProviderFaces, allowed: &[String]) {
    let allow = |seam: &str| allowed.is_empty() || allowed.iter().any(|key| key == seam);
    if faces.llm.is_some() && allow("seam.llm") {
        fiber.push_effect(Effect::BindSeam {
            name: "llm".to_owned(),
        });
    }
    if faces.embed.is_some() && allow("seam.embed") {
        fiber.push_effect(Effect::BindSeam {
            name: "embed".to_owned(),
        });
    }
    if faces.tts.is_some() && allow("seam.tts") {
        fiber.push_effect(Effect::BindSeam {
            name: "tts".to_owned(),
        });
    }
    if faces.stt.is_some() && allow("seam.stt") {
        fiber.push_effect(Effect::BindSeam {
            name: "stt".to_owned(),
        });
    }
}

async fn finish_asset_install_job(job: &mut AssetInstallJob) {
    drop(timeout(Duration::from_secs(2), job.conn.drain()).await);
    terminate_child(&mut job.child);
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
