use crate::host::{
    DelegationHost, SurfaceCallKind, UpgradeRequest, fold_brief, should_upgrade_steps,
    surface_call_kind,
};
use crate::types::UpgradeReason;
use async_trait::async_trait;
use ene_kernel::{KernelError, SurfaceRouter, SurfaceToolOutcome};
use ene_registry::{Layer, ToolRegistry};
use ene_session::{DelegationId, SoulId};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::sync::Arc;

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
            let reason = if kind == SurfaceCallKind::Upgrade {
                UpgradeReason::SideEffectTool
            } else {
                UpgradeReason::StepBudget
            };
            let learned = self.learned.lock().clone();
            let brief = fold_brief(&learned, Some(name));
            let goal = args
                .get("goal")
                .and_then(Value::as_str)
                .map_or_else(|| format!("continue after {name}"), str::to_owned);
            let job = self
                .host
                .auto_upgrade(UpgradeRequest {
                    soul_id: self.soul,
                    goal,
                    reason,
                    steps_so_far: learned.join("; "),
                    brief: Some(brief),
                    created_from_turn: None,
                })
                .map_err(|err| KernelError::Tool(err.to_string()))?;
            return Ok(SurfaceToolOutcome::Delegated {
                speech: "I'll look into that.".to_owned(),
                job_id: job.id.to_string(),
            });
        }
        match kind {
            SurfaceCallKind::Run => {
                let value = self
                    .registry
                    .execute(name, bind_soul_arg(args, self.soul), Layer::Surface)
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
}

impl JobLayerRouter {
    #[must_use]
    pub fn new(
        host: Arc<DelegationHost>,
        registry: Arc<ToolRegistry>,
        soul: SoulId,
        job_id: DelegationId,
    ) -> Self {
        Self {
            host,
            registry,
            soul,
            job_id,
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
        let value = self
            .registry
            .execute(
                name,
                bind_job_arg(args, self.soul, self.job_id, name),
                Layer::Job,
            )
            .await
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
        } else if name != "skill.load" && !object.contains_key("id") {
            object.insert("id".to_owned(), json!(job_id.to_string()));
        }
        if name == "artifact.register" && !object.contains_key("job_id") {
            object.insert("job_id".to_owned(), json!(job_id.to_string()));
        }
    }
    args
}
