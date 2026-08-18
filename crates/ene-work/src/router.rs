use crate::host::{
    DelegationHost, SurfaceCallKind, UpgradeRequest, fold_brief, should_upgrade_steps,
    surface_call_kind,
};
use crate::types::UpgradeReason;
use async_trait::async_trait;
use ene_kernel::{KernelError, SurfaceRouter, SurfaceToolOutcome};
use ene_registry::{Layer, ToolRegistry};
use ene_session::SoulId;
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

fn bind_soul_arg(mut args: Value, soul: SoulId) -> Value {
    if let Some(object) = args.as_object_mut() {
        object.insert("soul_id".to_owned(), json!(soul.to_string()));
    }
    args
}
