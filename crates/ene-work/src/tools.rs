use crate::error::WorkError;
use crate::host::{DelegationHost, StartDelegation};
use crate::skill::{catalog, load_skill, read_skill_file};
use crate::types::{Artifact, ArtifactKind, DelegationMode};
use crate::workflow::{BookmarkFill, fill_bookmark_job};
use async_trait::async_trait;
use chrono::Utc;
use ene_plane::Sensitivity;
use ene_registry::{ToolDefinition, ToolInvoke, ToolRegistry, ToolSource};
use ene_session::{DelegationId, SoulId};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Weak};
use uuid::Uuid;

/// Register surface `delegate.*` plus job-layer harness tools.
pub fn register_work_tools(
    registry: &Arc<ToolRegistry>,
    host: Arc<DelegationHost>,
    skills_home: PathBuf,
) {
    let invoke = Arc::new(WorkInvoker {
        host,
        skills_home,
        registry: Arc::downgrade(registry),
    });
    for def in delegate_defs() {
        registry.register_with(def, Arc::clone(&invoke) as Arc<dyn ToolInvoke>);
    }
}

fn delegate_defs() -> Vec<ToolDefinition> {
    vec![
        harness(
            "delegate.start",
            "Start an asynchronous task. Returns immediately.",
            json!({
                "type": "object",
                "properties": {
                    "goal": { "type": "string" },
                    "mode": { "type": "string" },
                    "title": { "type": "string" },
                    "soul_id": { "type": "string" },
                    "excerpt": { "type": "string" },
                    "parent_id": { "type": "string" }
                },
                "required": ["goal", "soul_id"]
            }),
            vec!["job.create".to_owned()],
        ),
        harness(
            "delegate.instruct",
            "Send a follow-up instruction that wakes the task.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "message": { "type": "string" }
                },
                "required": ["id", "message"]
            }),
            Vec::new(),
        ),
        harness(
            "delegate.message",
            "Share context without waking the task.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "message": { "type": "string" }
                },
                "required": ["id", "message"]
            }),
            Vec::new(),
        ),
        harness(
            "delegate.answer",
            "Answer a task question.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "answer": { "type": "string" }
                },
                "required": ["id", "answer"]
            }),
            Vec::new(),
        ),
        harness(
            "delegate.status",
            "Read task status without sending a message.",
            json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
            Vec::new(),
        ),
        harness(
            "delegate.cancel",
            "Cancel a running task.",
            json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
            Vec::new(),
        ),
        harness(
            "delegate.approve_plan",
            "Approve a task plan so mutating work may start.",
            json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
            Vec::new(),
        ),
        harness(
            "skill.load",
            "Load an installed skill body. Call skill.list first when the exact name is unknown.",
            json!({
                "type": "object",
                "properties": {
                    "soul_id": { "type": "string" },
                    "name": { "type": "string" }
                },
                "required": ["soul_id", "name"]
            }),
            Vec::new(),
        ),
        harness(
            "skill.list",
            "List skill names and descriptions available to a soul.",
            json!({
                "type": "object",
                "properties": { "soul_id": { "type": "string" } },
                "required": ["soul_id"]
            }),
            Vec::new(),
        ),
        harness(
            "skill.read",
            "Read a file from an installed skill package.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "path": { "type": "string" }
                },
                "required": ["name", "path"]
            }),
            Vec::new(),
        ),
        harness(
            "workflow.bookmark",
            "Research a theme and deliver a bookmark Markdown artifact.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "soul_id": { "type": "string" },
                    "theme": { "type": "string" },
                    "goal": { "type": "string" }
                },
                "required": ["id", "soul_id", "theme"]
            }),
            vec!["artifact.register".to_owned()],
        ),
        harness(
            "artifact.register",
            "Register a workspace file as an artifact.",
            json!({
                "type": "object",
                "properties": {
                    "soul_id": { "type": "string" },
                    "job_id": { "type": "string" },
                    "kind": { "type": "string" },
                    "title": { "type": "string" },
                    "path": { "type": "string" }
                },
                "required": ["soul_id", "job_id", "kind", "title", "path"]
            }),
            vec!["artifact.register".to_owned()],
        ),
        harness(
            "job.plan_write",
            "Update the workflow plan on a job.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "plan": { "type": "string" }
                },
                "required": ["id", "plan"]
            }),
            vec!["job.plan_write".to_owned()],
        ),
        harness(
            "delegation.send",
            "Child-to-parent mailbox send.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "kind": { "type": "string" },
                    "body": { "type": "string" },
                    "fraction": { "type": "number" }
                },
                "required": ["id", "kind", "body"]
            }),
            vec!["delegation.send".to_owned()],
        ),
    ]
}

fn harness(
    name: &str,
    description: &str,
    parameters: Value,
    side_effects: Vec<String>,
) -> ToolDefinition {
    ToolDefinition {
        name: name.to_owned(),
        description: description.to_owned(),
        parameters,
        output: json!({ "type": "object" }),
        side_effects,
        source: ToolSource::Harness {
            name: "work".to_owned(),
        },
        timeout_ms: Some(5_000),
        sensitivity: Sensitivity::None,
        category: String::new(),
        keywords: Vec::new(),
        examples: Vec::new(),
        background: false,
    }
}

struct WorkInvoker {
    host: Arc<DelegationHost>,
    skills_home: PathBuf,
    registry: Weak<ToolRegistry>,
}

#[async_trait]
impl ToolInvoke for WorkInvoker {
    async fn invoke(&self, name: &str, args: Value) -> Result<Value, String> {
        match name {
            "delegate.start" => start(self, &args),
            "delegate.instruct" => {
                self.host
                    .instruct(id_arg(&args)?, str_arg(&args, "message")?)
                    .map_err(|err| err.to_string())?;
                Ok(json!({ "ok": true }))
            }
            "delegate.message" => {
                self.host
                    .message(id_arg(&args)?, str_arg(&args, "message")?)
                    .map_err(|err| err.to_string())?;
                Ok(json!({ "ok": true }))
            }
            "delegate.answer" => {
                self.host
                    .answer(id_arg(&args)?, str_arg(&args, "answer")?)
                    .map_err(|err| err.to_string())?;
                Ok(json!({ "ok": true }))
            }
            "delegate.status" => {
                let job = self
                    .host
                    .status_snapshot(id_arg(&args)?)
                    .map_err(|err| err.to_string())?;
                Ok(json!({
                    "id": job.id.to_string(),
                    "status": job.status.as_str(),
                    "title": job.title,
                    "progress_note": job.progress_note,
                }))
            }
            "delegate.cancel" => match self.host.cancel(id_arg(&args)?) {
                Ok(status) => Ok(json!({ "status": status.as_str() })),
                Err(WorkError::AlreadyCompleted) => Ok(json!({ "status": "already_completed" })),
                Err(WorkError::Cancelled) => Ok(json!({ "status": "cancelled" })),
                Err(other) => Err(other.to_string()),
            },
            "delegate.approve_plan" => {
                self.host
                    .approve_plan(id_arg(&args)?)
                    .map_err(|err| err.to_string())?;
                Ok(json!({ "ok": true }))
            }
            "skill.load" => {
                let soul_id = soul_arg(&args)?;
                let skill_name = args
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "invalid_arguments: missing name".to_owned())?;
                let enabled = soul_skill_refs(&self.host, soul_id);
                let available =
                    catalog(&self.skills_home, &enabled).map_err(|err| err.to_string())?;
                if !available.iter().any(|(name, _)| name == skill_name) {
                    return Err(unknown_skill_message(skill_name, &available));
                }
                let meta = load_skill(&self.skills_home, skill_name).map_err(|err| match err {
                    WorkError::UnknownSkill(_) => unknown_skill_message(skill_name, &available),
                    other => other.to_string(),
                })?;
                Ok(json!({
                    "name": meta.name,
                    "description": meta.description,
                    "body": meta.body,
                }))
            }
            "skill.list" => {
                let soul_id = soul_arg(&args)?;
                let enabled = soul_skill_refs(&self.host, soul_id);
                let rows = catalog(&self.skills_home, &enabled).map_err(|err| err.to_string())?;
                Ok(json!({
                    "skills": rows
                        .into_iter()
                        .map(|(name, description)| {
                            json!({ "name": name, "description": description })
                        })
                        .collect::<Vec<_>>(),
                }))
            }
            "skill.read" => {
                let text = read_skill_file(
                    &self.skills_home,
                    str_arg(&args, "name")?,
                    str_arg(&args, "path")?,
                )
                .map_err(|err| err.to_string())?;
                Ok(json!({ "text": text }))
            }
            "workflow.bookmark" => {
                let theme = args
                    .get("theme")
                    .and_then(Value::as_str)
                    .or_else(|| args.get("goal").and_then(Value::as_str))
                    .ok_or_else(|| "missing theme".to_owned())?;
                let soul_id = soul_arg(&args)?;
                let enabled = soul_skill_refs(&self.host, soul_id);
                let registry = self.registry.upgrade();
                let (artifact, report) = fill_bookmark_job(BookmarkFill {
                    host: self.host.as_ref(),
                    soul_id,
                    job_id: id_arg(&args)?,
                    theme,
                    skills_home: &self.skills_home,
                    enabled: &enabled,
                    registry: registry.as_deref(),
                })
                .await
                .map_err(|err| err.to_string())?;
                Ok(json!({
                    "artifact_id": artifact.id,
                    "path": artifact.path,
                    "speech": report.speech,
                }))
            }
            "artifact.register" => {
                let job_id = job_id_arg(&args)?;
                self.host
                    .require_mutating_allowed(job_id)
                    .map_err(|err| err.to_string())?;
                register_artifact_via_host(self.host.as_ref(), &args)
            }
            "job.plan_write" => {
                let report = self
                    .host
                    .present_plan(id_arg(&args)?, str_arg(&args, "plan")?)
                    .map_err(|err| err.to_string())?;
                Ok(json!({ "ok": true, "speech": report.speech }))
            }
            "delegation.send" => send_from_child(&self.host, &args),
            other => Err(format!("unknown work tool {other}")),
        }
    }
}

fn unknown_skill_message(skill_name: &str, available: &[(String, String)]) -> String {
    let mut names: Vec<&str> = available.iter().map(|(name, _)| name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    format!(
        "unknown_skill: unknown skill {skill_name}; call skill.list to discover installed skills{}",
        if names.is_empty() {
            String::new()
        } else {
            format!(" (available skills: {})", names.join(", "))
        }
    )
}

fn start(invoker: &WorkInvoker, args: &Value) -> Result<Value, String> {
    let mode = if args.get("mode").and_then(Value::as_str) == Some("internal") {
        DelegationMode::Internal
    } else {
        DelegationMode::Public
    };
    let parent_id = args
        .get("parent_id")
        .and_then(Value::as_str)
        .map(DelegationId::from_str)
        .transpose()
        .map_err(|err| err.to_string())?;
    let depth = if let Some(parent_id) = parent_id {
        let parent_depth = invoker
            .host
            .store()
            .delegation_depth(parent_id)
            .map_err(|err| err.to_string())?
            .unwrap_or(0);
        parent_depth + 1
    } else {
        args.get("depth")
            .and_then(Value::as_u64)
            .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
    };
    let job = invoker
        .host
        .start(StartDelegation {
            soul_id: soul_arg(args)?,
            goal: str_arg(args, "goal")?.to_owned(),
            mode,
            title: args.get("title").and_then(Value::as_str).map(str::to_owned),
            brief: args
                .get("excerpt")
                .and_then(Value::as_str)
                .map(str::to_owned),
            plan: None,
            created_from_turn: None,
            depth,
            parent_id,
            success_criteria: args
                .get("success_criteria")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            allowed_tools: args
                .get("allowed_tools")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
        })
        .map_err(|err| err.to_string())?;
    Ok(json!({
        "delegation_id": job.id.to_string(),
        "status": job.status.as_str(),
        "accepted": true,
    }))
}

fn register_artifact_via_host(host: &DelegationHost, args: &Value) -> Result<Value, String> {
    let kind = ArtifactKind::try_parse(str_arg(args, "kind")?).map_err(|err| err.to_string())?;
    let job_id = job_id_arg(args)?;
    let path = str_arg(args, "path")?.to_owned();
    let art = host
        .register_artifact_for_job(
            job_id,
            Artifact {
                id: Uuid::now_v7().to_string(),
                soul_id: soul_arg(args)?,
                job_id: Some(job_id),
                kind,
                title: str_arg(args, "title")?.to_owned(),
                path,
                mime: None,
                size_bytes: None,
                created_at: Utc::now().to_rfc3339(),
                delivered: false,
            },
        )
        .map_err(|err| err.to_string())?;
    Ok(json!({ "id": art.id }))
}

fn send_from_child(host: &DelegationHost, args: &Value) -> Result<Value, String> {
    let id = id_arg(args)?;
    let kind = str_arg(args, "kind")?;
    let body = str_arg(args, "body")?;
    match kind {
        "progress" => {
            let fraction = args
                .get("fraction")
                .and_then(Value::as_f64)
                .map(|n| n as f32);
            let report = host
                .progress(id, fraction, body)
                .map_err(|err| err.to_string())?;
            Ok(json!({ "speech": report.speech }))
        }
        "question" => {
            let report = host.question(id, body).map_err(|err| err.to_string())?;
            Ok(json!({ "speech": report.speech }))
        }
        "verifying" => {
            host.begin_verifying(id).map_err(|err| err.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "complete" => {
            let report = host.complete(id, body).map_err(|err| err.to_string())?;
            Ok(json!({ "speech": report.speech }))
        }
        "failed" => {
            let report = host.fail(id, body).map_err(|err| err.to_string())?;
            Ok(json!({ "speech": report.speech }))
        }
        other => {
            host.store()
                .mailbox_push(id, "child_to_parent", other, body)
                .map_err(|err| err.to_string())?;
            Ok(json!({ "ok": true }))
        }
    }
}

fn soul_skill_refs(host: &DelegationHost, soul: SoulId) -> Vec<String> {
    ene_companion::CompanionStore::open(host.data_dir().join("companions.db"))
        .ok()
        .and_then(|store| store.get_soul(soul).ok().flatten())
        .map(|row| row.skill_refs)
        .unwrap_or_default()
}

fn soul_arg(args: &Value) -> Result<SoulId, String> {
    let raw = args
        .get("soul_id")
        .and_then(Value::as_str)
        .ok_or("missing soul_id")?;
    SoulId::from_str(raw).map_err(|err| err.to_string())
}

fn id_arg(args: &Value) -> Result<DelegationId, String> {
    let raw = args.get("id").and_then(Value::as_str).ok_or("missing id")?;
    DelegationId::from_str(raw).map_err(|err| err.to_string())
}

fn job_id_arg(args: &Value) -> Result<DelegationId, String> {
    let raw = args
        .get("job_id")
        .and_then(Value::as_str)
        .ok_or("missing job_id")?;
    DelegationId::from_str(raw).map_err(|err| err.to_string())
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing {key}"))
}

/// `delegate.*` must appear on the surface schema; `delegation.send` must not.
#[must_use]
pub fn surface_shows_delegate(registry: &ToolRegistry) -> bool {
    let names: Vec<_> = registry
        .schemas(ene_registry::Layer::Surface)
        .iter()
        .filter_map(|schema| {
            schema
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    names.iter().any(|n| n == "delegate.start") && !names.iter().any(|n| n == "delegation.send")
}
