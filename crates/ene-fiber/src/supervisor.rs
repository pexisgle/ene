use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ene_plugin_ipc::{BuiltinKind, HostConn, ToolCall};
use ene_registry::{
    Layer, ToolDefinition, ToolInvoke, ToolRegistry, ToolSource, builtin_digest, definitions_for,
};
use parking_lot::Mutex;
use serde_json::Value;
use thiserror::Error;
use tokio::time::timeout;
use uuid::Uuid;

use crate::broker::Broker;
use crate::fiber::{Effect, Fiber, FiberState, FiberUid};
use crate::spawn::{SpawnOpts, SpawnedPlugin, spawn_plugin};

/// Profile row (manifest subset used at W1).
#[derive(Debug, Clone)]
pub struct ProfileRow {
    pub row_id: String,
    pub plugin: String,
    pub requires: Vec<String>,
    pub capabilities: Vec<String>,
    pub sandbox_required: bool,
}

/// Fiber supervisor. Reconcile is per-row; the core process is not restarted.
pub struct Supervisor {
    fibers: Mutex<HashMap<String, Fiber>>,
    children: Mutex<HashMap<String, Child>>,
    sessions: Mutex<HashMap<String, Arc<PluginSession>>>,
    registry: Arc<ToolRegistry>,
    broker: Mutex<Broker>,
    workspace: PathBuf,
}

struct PluginSession {
    conn: tokio::sync::Mutex<HostConn<tokio::net::UnixStream>>,
}

struct PluginInvoker {
    session: Arc<PluginSession>,
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
            .await
            .map_err(|err| err.to_string())?;
        if result.status == "ok" {
            Ok(result.value)
        } else {
            Err(result.value.to_string())
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
    #[error("spawn: {0}")]
    Spawn(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Ipc(#[from] ene_plugin_ipc::IpcError),
}

impl Supervisor {
    #[must_use]
    pub fn new(workspace: PathBuf, registry: Arc<ToolRegistry>) -> Self {
        Self {
            fibers: Mutex::new(HashMap::new()),
            children: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            broker: Mutex::new(Broker::new(workspace.clone())),
            registry,
            workspace,
        }
    }

    #[must_use]
    pub fn registry(&self) -> Arc<ToolRegistry> {
        Arc::clone(&self.registry)
    }

    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.workspace
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
            self.registry.register(def);
        }
        {
            let mut broker = self.broker.lock();
            for cap in &row.capabilities {
                broker.grant(fiber.uid, cap.clone());
                fiber.push_effect(Effect::BrokerGrant { op: cap.clone() });
            }
        }
        finish_active(&mut fiber);
        let uid = fiber.uid;
        self.fibers.lock().insert(row.row_id.clone(), fiber);
        Ok(uid)
    }

    /// Spawn the plugin binary, handshake, and register tools from `spec`.
    pub async fn activate_process(
        &self,
        row: &ProfileRow,
        binary: &Path,
    ) -> Result<FiberUid, SupervisorError> {
        if row.sandbox_required && !ene_sandbox::supported() {
            return Err(SupervisorError::SandboxRequired);
        }
        let kind = plugin_kind(&row.plugin)
            .ok_or_else(|| SupervisorError::UnknownPlugin(row.plugin.clone()))?;
        let mut fiber = Fiber::new(&row.row_id, &row.plugin);
        fiber.requires.clone_from(&row.requires);
        fiber.sandbox_required = row.sandbox_required;
        fiber.state = FiberState::Loading;
        let spawned = match spawn_plugin(SpawnOpts {
            binary,
            plugin_id: kind.plugin_id(),
            digest: &builtin_digest(kind),
            socket_dir: &self.workspace.join("sockets"),
            row_id: &row.row_id,
            sandbox_required: row.sandbox_required,
            temp_dir: &self.workspace.join("plugin-tmp").join(&row.row_id),
            workspace: &self.workspace,
        })
        .await
        {
            Ok(spawned) => spawned,
            Err(err) => {
                self.rollback_loading(&fiber);
                return Err(err);
            }
        };
        if let Err(err) = self.apply_spawned(row, kind, &mut fiber, spawned).await {
            self.rollback_loading(&fiber);
            return Err(err);
        }
        let uid = fiber.uid;
        self.fibers.lock().insert(row.row_id.clone(), fiber);
        Ok(uid)
    }

    async fn apply_spawned(
        &self,
        row: &ProfileRow,
        kind: BuiltinKind,
        fiber: &mut Fiber,
        spawned: SpawnedPlugin,
    ) -> Result<(), SupervisorError> {
        let SpawnedPlugin { child, mut conn } = spawned;
        let pid = child.id();
        fiber.push_effect(Effect::SpawnProcess { pid });
        self.children.lock().insert(row.row_id.clone(), child);
        let tools = conn.list_tools().await?;
        let session = Arc::new(PluginSession {
            conn: tokio::sync::Mutex::new(conn),
        });
        self.sessions
            .lock()
            .insert(row.row_id.clone(), Arc::clone(&session));
        let invoke: Arc<dyn ToolInvoke> = Arc::new(PluginInvoker {
            session: Arc::clone(&session),
        });
        let source = ToolSource::Plugin {
            plugin_id: kind.plugin_id().to_owned(),
        };
        for spec in tools {
            let def = ToolDefinition::from_wire(spec, source.clone());
            fiber.push_effect(Effect::RegisterTool {
                name: def.name.clone(),
            });
            self.registry.register_with(def, Arc::clone(&invoke));
        }
        {
            let mut broker = self.broker.lock();
            for cap in &row.capabilities {
                broker.grant(fiber.uid, cap.clone());
                fiber.push_effect(Effect::BrokerGrant { op: cap.clone() });
            }
        }
        finish_active(fiber);
        Ok(())
    }

    fn rollback_loading(&self, fiber: &Fiber) {
        if let Some(kind) = plugin_kind(&fiber.plugin) {
            self.registry.unregister_source(&ToolSource::Plugin {
                plugin_id: kind.plugin_id().to_owned(),
            });
        }
        self.broker.lock().revoke_all(fiber.uid);
        if let Some(mut child) = self.children.lock().remove(&fiber.row_id) {
            terminate_child(&mut child);
        }
        self.sessions.lock().remove(&fiber.row_id);
    }

    /// Unload a row: stop providing, then apply dispose LIFO (I-46).
    pub async fn unload(&self, row_id: &str) {
        let Some(mut fiber) = self.fibers.lock().remove(row_id) else {
            return;
        };
        fiber.state = FiberState::Unloading;
        let session = self.sessions.lock().remove(row_id);
        if let Some(session) = session {
            let drain = async { session.conn.lock().await.drain().await };
            drop(timeout(Duration::from_secs(2), drain).await);
        }
        if let Some(mut child) = self.children.lock().remove(row_id) {
            terminate_child(&mut child);
        }
        if let Some(kind) = plugin_kind(&fiber.plugin) {
            self.registry.unregister_source(&ToolSource::Plugin {
                plugin_id: kind.plugin_id().to_owned(),
            });
        }
        self.broker.lock().revoke_all(fiber.uid);
        fiber.dispose.clear();
        fiber.state = FiberState::Inactive;
    }

    /// Disable one row; other rows keep uid and Active (I-49).
    pub async fn disable_row(&self, row_id: &str) {
        self.unload(row_id).await;
    }

    #[must_use]
    pub fn fiber(&self, row_id: &str) -> Option<Fiber> {
        self.fibers.lock().get(row_id).cloned()
    }

    #[must_use]
    pub fn active_row_ids(&self) -> Vec<String> {
        self.fibers
            .lock()
            .iter()
            .filter(|(_, fiber)| fiber.state == FiberState::Active)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Snapshot of every profile row the supervisor currently tracks.
    #[must_use]
    pub fn list_fibers(&self) -> Vec<Fiber> {
        self.fibers.lock().values().cloned().collect()
    }

    #[must_use]
    pub fn broker_has_grant(&self, uid: FiberUid, op: &str) -> bool {
        self.broker.lock().has_grant(uid, op)
    }

    pub fn broker_fs_read(&self, uid: FiberUid, path: &Path) -> Result<String, crate::BrokerError> {
        self.broker.lock().fs_read(uid, path)
    }

    #[must_use]
    pub fn surface_has_tool(&self, name: &str) -> bool {
        self.registry
            .schemas(Layer::Surface)
            .iter()
            .any(|schema| schema.get("name").and_then(|v| v.as_str()) == Some(name))
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        for (_, mut child) in self.children.lock().drain() {
            terminate_child(&mut child);
        }
    }
}

fn finish_active(fiber: &mut Fiber) {
    fiber.provides = fiber
        .dispose
        .iter()
        .filter_map(|effect| match effect {
            Effect::RegisterTool { name } => Some(format!("tool.{name}")),
            Effect::BrokerGrant { op } => Some(format!("broker.{op}")),
            Effect::SpawnProcess { .. } => None,
        })
        .collect();
    fiber.state = FiberState::Active;
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
        _ => None,
    }
}

fn row_needs_os_sandbox(plugin: &str) -> bool {
    matches!(plugin, "tool.fs" | "tool.exec" | "tool.web")
}
