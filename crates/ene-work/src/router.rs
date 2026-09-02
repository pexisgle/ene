use crate::host::{
    DelegationHost, StartDelegation, SurfaceCallKind, fold_brief, should_upgrade_steps,
    surface_call_kind,
};
use crate::types::{DelegationMode, NewToolExecution, ToolExecStatus};
use async_trait::async_trait;
use ene_kernel::{KernelError, SurfaceRouter, SurfaceToolOutcome};
use ene_session::{DelegationId, SoulId};
use ene_tool_registry::{Layer, ToolRegistry, ToolSource};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Dialogue-lane router: empty-`side_effects` tools run; anything else upgrades.
pub struct WorkSurfaceRouter {
    host: Arc<DelegationHost>,
    registry: Arc<ToolRegistry>,
    soul: SoulId,
    max_steps: u32,
    learned: Mutex<Vec<String>>,
}

impl WorkSurfaceRouter {
    #[must_use]
    pub fn new(
        host: Arc<DelegationHost>,
        registry: Arc<ToolRegistry>,
        soul: SoulId,
        max_steps: u32,
    ) -> Self {
        Self {
            host,
            registry,
            soul,
            max_steps,
            learned: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl SurfaceRouter for WorkSurfaceRouter {
    async fn on_tool(
        &self,
        name: &str,
        args: Value,
        step: u32,
    ) -> Result<SurfaceToolOutcome, KernelError> {
        let kind = surface_call_kind(&self.registry, name);
        let budget = should_upgrade_steps(step, self.max_steps);
        if budget || kind == SurfaceCallKind::Upgrade {
            let learned = self.learned.lock().clone();
            let brief = fold_brief(&learned, Some(name));
            let goal = args
                .get("goal")
                .and_then(Value::as_str)
                .map_or_else(|| format!("continue after {name}"), str::to_owned);
            let value = self
                .registry
                .execute(
                    "delegate.start",
                    json!({
                        "goal": goal,
                        "mode": "public",
                        "soul_id": self.soul.to_string(),
                        "title": "task",
                        "excerpt": brief,
                    }),
                    Layer::Surface,
                )
                .await
                .map_err(|err| KernelError::Tool(err.to_string()))?;
            let job_id = value
                .get("delegation_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    KernelError::Tool("delegate.start did not return a delegation_id".to_owned())
                })?;
            let speech = if kind == SurfaceCallKind::Upgrade {
                format!("That action requires a Work job, so I started one for `{name}`.")
            } else {
                "I'll look into that.".to_owned()
            };
            return Ok(SurfaceToolOutcome::Delegated {
                speech,
                job_id: job_id.to_owned(),
            });
        }
        match kind {
            SurfaceCallKind::Run => {
                // One id per model call: approval popups and transcript entries
                // must reference this exact invocation, not a later retry.
                let call_id = Uuid::now_v7().to_string();
                if self.registry.get(name).is_some_and(|def| def.background) {
                    return start_background_outcome(BgStart {
                        host: &self.host,
                        registry: &self.registry,
                        soul: self.soul,
                        job_id: None,
                        name,
                        args: bind_soul_arg(args, self.soul),
                        layer: Layer::Surface,
                        workspace: None,
                        call_id: &call_id,
                    })
                    .await;
                }
                let value = self
                    .registry
                    .execute_call(
                        name,
                        bind_soul_arg(args, self.soul),
                        Layer::Surface,
                        &call_id,
                    )
                    .await
                    .map_err(|err| KernelError::Tool(err.to_string()))?;
                self.learned.lock().push(name.to_owned());
                Ok(SurfaceToolOutcome::Result(value))
            }
            SurfaceCallKind::Unknown => Err(KernelError::Tool(format!("unknown tool {name}"))),
            SurfaceCallKind::Upgrade => Err(KernelError::Tool(format!(
                "side-effect tool {name} must not run on the surface"
            ))),
        }
    }
}

/// Job-lane router: side-effect tools run here (plan-gated). Never upgrades.
pub struct JobLayerRouter {
    host: Arc<DelegationHost>,
    registry: Arc<ToolRegistry>,
    soul: SoulId,
    job_id: DelegationId,
    workspace_dir: PathBuf,
}

impl JobLayerRouter {
    #[must_use]
    pub fn new(
        host: Arc<DelegationHost>,
        registry: Arc<ToolRegistry>,
        soul: SoulId,
        job_id: DelegationId,
        workspace_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            host,
            registry,
            soul,
            job_id,
            workspace_dir: workspace_dir.into(),
        }
    }
}

#[async_trait]
impl SurfaceRouter for JobLayerRouter {
    async fn on_tool(
        &self,
        name: &str,
        args: Value,
        _step: u32,
    ) -> Result<SurfaceToolOutcome, KernelError> {
        let Some(def) = self.registry.get(name) else {
            return Err(KernelError::Tool(format!("unknown tool {name}")));
        };
        if needs_plan(name, &def.side_effects) {
            self.host
                .require_mutating_allowed(self.job_id)
                .map_err(|err| KernelError::Tool(err.to_string()))?;
        }
        if def.background {
            let call_id = Uuid::now_v7().to_string();
            return start_background_outcome(BgStart {
                host: &self.host,
                registry: &self.registry,
                soul: self.soul,
                job_id: Some(self.job_id),
                name,
                args: bind_job_arg(args, self.soul, self.job_id, name),
                layer: Layer::Job,
                workspace: Some(self.workspace_dir.as_path()),
                call_id: &call_id,
            })
            .await;
        }
        let bound = bind_job_arg(args, self.soul, self.job_id, name);
        let value = if name == "delegation.send" {
            self.registry.execute_host(name, bound).await
        } else {
            self.registry
                .execute_in_workspace(name, bound, Layer::Job, &self.workspace_dir)
                .await
        }
        .map_err(|err| KernelError::Tool(err.to_string()))?;
        Ok(SurfaceToolOutcome::Result(value))
    }
}

fn needs_plan(name: &str, side_effects: &[String]) -> bool {
    !side_effects.is_empty()
        && !name.starts_with("delegate.")
        && name != "delegation.send"
        && name != "job.plan_write"
}

fn bind_soul_arg(mut args: Value, soul: SoulId) -> Value {
    if let Some(object) = args.as_object_mut() {
        object.insert("soul_id".to_owned(), json!(soul.to_string()));
    }
    args
}

fn bind_job_arg(mut args: Value, soul: SoulId, job_id: DelegationId, name: &str) -> Value {
    if let Some(object) = args.as_object_mut() {
        object.insert("soul_id".to_owned(), json!(soul.to_string()));
        if name == "delegate.start" {
            object
                .entry("parent_id")
                .or_insert_with(|| json!(job_id.to_string()));
        } else if name != "skill.load" {
            object.insert("id".to_owned(), json!(job_id.to_string()));
        }
        if name == "artifact.register" {
            object.insert("job_id".to_owned(), json!(job_id.to_string()));
        }
    }
    args
}

struct BgStart<'a> {
    host: &'a Arc<DelegationHost>,
    registry: &'a Arc<ToolRegistry>,
    soul: SoulId,
    job_id: Option<DelegationId>,
    name: &'a str,
    args: Value,
    layer: Layer,
    workspace: Option<&'a Path>,
    call_id: &'a str,
}

async fn start_background_outcome(start: BgStart<'_>) -> Result<SurfaceToolOutcome, KernelError> {
    let def = start
        .registry
        .get(start.name)
        .ok_or_else(|| KernelError::Tool(format!("unknown tool {}", start.name)))?;
    // Approval must hold the dispatch, not undo it: authorize before any job
    // row or execution record exists so Deny leaves no side effects behind.
    start
        .registry
        .authorize_background(
            start.name,
            &start.args,
            start.layer,
            start.workspace,
            start.call_id,
        )
        .await
        .map_err(|err| KernelError::Tool(err.to_string()))?;
    let job_id = if let Some(job_id) = start.job_id {
        job_id
    } else {
        start
            .host
            .start(StartDelegation {
                soul_id: start.soul,
                goal: format!("background {}", start.name),
                mode: DelegationMode::Public,
                title: Some(format!("bg:{}", start.name)),
                brief: Some(format!("background tool {}", start.name)),
                plan: None,
                created_from_turn: None,
                depth: 0,
                parent_id: None,
                success_criteria: Vec::new(),
                allowed_tools: Vec::new(),
            })
            .map_err(|err| KernelError::Tool(err.to_string()))?
            .id
    };
    let execution_id = Uuid::now_v7().to_string();
    let plugin_id = match def.source {
        ToolSource::Plugin { plugin_id } => Some(plugin_id),
        ToolSource::Mcp { server } => Some(format!("mcp.{server}")),
        ToolSource::Harness { name } => Some(name),
    };
    start
        .host
        .begin_tool_execution(&NewToolExecution {
            execution_id: execution_id.clone(),
            job_id: Some(job_id),
            soul_id: start.soul,
            tool_name: start.name.to_owned(),
            plugin_id,
            call_id: start.call_id.to_owned(),
        })
        .map_err(|err| KernelError::Tool(err.to_string()))?;
    let started = start
        .registry
        .start_background_pre_authorized(
            start.name,
            start.args,
            &execution_id,
            start.layer,
            def.timeout_ms.map(u64::from),
            start.workspace,
        )
        .await;
    if let Err(err) = started {
        drop(
            start
                .host
                .crash_tool_execution(&execution_id, "start_failed"),
        );
        return Err(KernelError::Tool(err.to_string()));
    }
    spawn_execution_watch(
        Arc::clone(start.host),
        Arc::clone(start.registry),
        start.name.to_owned(),
        execution_id.clone(),
        def.timeout_ms,
    );
    Ok(SurfaceToolOutcome::Result(json!({
        "execution_id": execution_id,
        "job_id": job_id.to_string(),
        "call_id": start.call_id,
        "status": "started",
    })))
}

fn spawn_execution_watch(
    host: Arc<DelegationHost>,
    registry: Arc<ToolRegistry>,
    name: String,
    execution_id: String,
    timeout_ms: Option<u32>,
) {
    tokio::spawn(async move {
        let limit = timeout_ms.map_or(Duration::from_mins(5), |ms| {
            Duration::from_millis(u64::from(ms))
        });
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            if tokio::time::Instant::now() >= deadline {
                drop(host.timeout_tool_execution(&execution_id));
                drop(registry.cancel_background(&name, &execution_id).await);
                break;
            }
            match registry.take_completion(&name, &execution_id).await {
                Ok(Some(complete)) => {
                    let status = match complete.status.as_str() {
                        "cancelled" => ToolExecStatus::Cancelled,
                        "timed_out" => ToolExecStatus::TimedOut,
                        "error" => ToolExecStatus::Failed,
                        _ => ToolExecStatus::Completed,
                    };
                    let summary = complete.value.to_string();
                    drop(host.apply_tool_completion(
                        &execution_id,
                        status,
                        complete.error_class.as_deref(),
                        &summary,
                    ));
                    break;
                }
                Ok(None) => {}
                Err(_) => {
                    drop(host.crash_tool_execution(&execution_id, "plugin_crash"));
                    break;
                }
            }
            if let Ok((phase, error_class)) = registry.status_background(&name, &execution_id).await
            {
                let Some(status) = terminal_phase(&phase) else {
                    tokio::time::sleep(Duration::from_millis(40)).await;
                    continue;
                };
                let summary = error_class.clone().unwrap_or(phase);
                drop(host.apply_tool_completion(
                    &execution_id,
                    status,
                    error_class.as_deref(),
                    &summary,
                ));
                break;
            }
            drop(host.crash_tool_execution(&execution_id, "plugin_crash"));
            break;
        }
    });
}

fn terminal_phase(phase: &str) -> Option<ToolExecStatus> {
    match phase {
        "completed" => Some(ToolExecStatus::Completed),
        "failed" => Some(ToolExecStatus::Failed),
        "cancelled" => Some(ToolExecStatus::Cancelled),
        "timed_out" => Some(ToolExecStatus::TimedOut),
        "plugin_crash" => Some(ToolExecStatus::PluginCrash),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_job_arg_overwrites_model_supplied_id() {
        let soul = SoulId::new();
        let job = DelegationId::new();
        let foreign = DelegationId::new();
        let bound = bind_job_arg(
            json!({"id": foreign.to_string(), "kind": "complete"}),
            soul,
            job,
            "delegation.send",
        );
        assert_eq!(bound["id"], json!(job.to_string()));
        assert_eq!(bound["soul_id"], json!(soul.to_string()));
        let plan = bind_job_arg(
            json!({"id": foreign.to_string(), "plan": "steps"}),
            soul,
            job,
            "job.plan_write",
        );
        assert_eq!(plan["id"], json!(job.to_string()));
        let artifact = bind_job_arg(
            json!({"job_id": foreign.to_string()}),
            soul,
            job,
            "artifact.register",
        );
        assert_eq!(artifact["job_id"], json!(job.to_string()));
    }

    #[test]
    fn bind_job_arg_leaves_skill_load_id() {
        let soul = SoulId::new();
        let job = DelegationId::new();
        let bound = bind_job_arg(json!({"id": "research"}), soul, job, "skill.load");
        assert_eq!(bound["id"], json!("research"));
    }
}
