use crate::host::{
    DelegationHost, StartDelegation, SurfaceCallKind, UpgradeRequest, question_timed_out,
    should_upgrade_steps, surface_call_kind,
};
use crate::mcp::{McpProfile, McpTool, ScriptedMcp, register_mcp_tools};
use crate::observe::{ObservationPipeline, ObserveAction, contains_raw_screenshot};
use crate::router::WorkSurfaceRouter;
use crate::runner::{JobDrive, drive_job};
use crate::schedule::{QuietWindow, catch_up_missed, fire_due, reminder_report};
use crate::skill::{
    catalog, install_skill_dir, load_skill, match_skills, parse_skill_md, read_skill_file,
    skill_active_blocks, skill_catalog_blocks, skill_emotion_notes, skill_proactive_hints,
};
use crate::store::WorkStore;
use crate::tools::{register_work_tools, surface_shows_delegate};
use crate::types::{
    Artifact, ArtifactKind, DelegationMode, JobStatus, NewSchedule, ScheduleAction, UpgradeReason,
};
use crate::vision::{
    PlaceholderScreenshot, PngScreenshot, ScreenshotError, capture_screenshot, observe_screen,
    register_screenshot_tool, screenshot_is_job_or_surface, screenshot_png,
};
use async_trait::async_trait;
use chrono::{Duration, TimeZone, Utc};
use ene_companion::{WorldStateMemory, WorldStateSettings};
use ene_kernel::{
    ConversationModel, DisplayDepth, EchoModel, EventKind, HarnessSettings, KernelError,
    LaneHandle, LaneMindSettings, LaneOptions, ModelGeneration, ModelRequest, SurfaceRouter,
    SurfaceToolOutcome, ToolCall,
};
use ene_plane::Sensitivity;
use ene_registry::{BuiltinInvoker, Layer, ToolDefinition, ToolInvoke, ToolRegistry, ToolSource};
use ene_session::{
    NewSession, SessionCreatedBy, SessionKind, SessionStore, SoulId, derive_messages,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration as StdDuration;
use tempfile::TempDir;

struct SpyInvoke {
    hit: Arc<AtomicBool>,
}

struct CaptureInvoke {
    last: parking_lot::Mutex<Option<Value>>,
}

struct SearchInvoke;

#[async_trait]
impl ToolInvoke for SpyInvoke {
    async fn invoke(&self, _name: &str, _args: Value) -> Result<Value, String> {
        self.hit.store(true, Ordering::SeqCst);
        Ok(json!({ "ok": true }))
    }
}

struct CountingInvoke {
    hit: Arc<AtomicBool>,
}

#[async_trait]
impl ToolInvoke for CountingInvoke {
    async fn invoke(&self, name: &str, args: Value) -> Result<Value, String> {
        self.hit.store(true, Ordering::SeqCst);
        BuiltinInvoker.invoke(name, args).await
    }
}

#[async_trait]
impl ToolInvoke for CaptureInvoke {
    async fn invoke(&self, _name: &str, args: Value) -> Result<Value, String> {
        *self.last.lock() = Some(args);
        Ok(json!({ "ok": true }))
    }
}

#[async_trait]
impl ToolInvoke for SearchInvoke {
    async fn invoke(&self, _name: &str, _args: Value) -> Result<Value, String> {
        Ok(json!({
            "results": [{
                "title": "Tokyo",
                "snippet": "Shibuya crossing",
                "url": "https://example.invalid/tokyo"
            }]
        }))
    }
}

struct SequenceModel {
    remaining: parking_lot::Mutex<Vec<ModelGeneration>>,
}

struct CapturePrompt {
    prompts: parking_lot::Mutex<Vec<String>>,
    remaining: parking_lot::Mutex<Vec<ModelGeneration>>,
}

#[async_trait]
impl ConversationModel for CapturePrompt {
    async fn generate(&self, request: ModelRequest) -> Result<ModelGeneration, KernelError> {
        self.prompts.lock().extend(
            request
                .messages
                .iter()
                .map(ene_session::ProjectedMessage::text),
        );
        let mut remaining = self.remaining.lock();
        remaining.pop().map_or_else(
            || {
                Ok(ModelGeneration {
                    text: "all done".to_owned(),
                    ..ModelGeneration::default()
                })
            },
            Ok,
        )
    }
}

#[async_trait]
impl ConversationModel for SequenceModel {
    async fn generate(&self, _request: ModelRequest) -> Result<ModelGeneration, KernelError> {
        let mut remaining = self.remaining.lock();
        remaining.pop().map_or_else(
            || {
                Ok(ModelGeneration {
                    text: "all done".to_owned(),
                    ..ModelGeneration::default()
                })
            },
            Ok,
        )
    }
}

fn fs_write_def() -> ToolDefinition {
    ToolDefinition {
        name: "fs.write".to_owned(),
        description: "Write a file".to_owned(),
        parameters: json!({"type":"object"}),
        output: json!({"type":"object"}),
        side_effects: vec!["fs.write".to_owned()],
        source: ToolSource::Harness {
            name: "fs".to_owned(),
        },
        timeout_ms: Some(1_000),
        sensitivity: Sensitivity::None,
        category: String::new(),
        keywords: Vec::new(),
        examples: Vec::new(),
        background: false,
    }
}

fn utility_time_def() -> ToolDefinition {
    ToolDefinition {
        name: "utility.time".to_owned(),
        description: "Current UTC time".to_owned(),
        parameters: json!({"type":"object"}),
        output: json!({"type":"object"}),
        side_effects: Vec::new(),
        source: ToolSource::Harness {
            name: "utility".to_owned(),
        },
        timeout_ms: Some(1_000),
        sensitivity: Sensitivity::None,
        category: String::new(),
        keywords: Vec::new(),
        examples: Vec::new(),
        background: false,
    }
}

fn open_work() -> (TempDir, Arc<WorkStore>, Arc<DelegationHost>, SoulId) {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(WorkStore::open(dir.path().join("companions.db")).unwrap());
    let host = Arc::new(DelegationHost::new(
        Arc::clone(&store),
        dir.path().to_path_buf(),
    ));
    (dir, store, host, SoulId::new())
}

fn public_start(host: &DelegationHost, soul: SoulId, goal: &str) -> crate::types::Job {
    host.start(StartDelegation {
        soul_id: soul,
        goal: goal.to_owned(),
        mode: DelegationMode::Public,
        title: Some(goal.to_owned()),
        brief: None,
        plan: None,
        created_from_turn: None,
        depth: 0,
        parent_id: None,
    })
    .unwrap()
}

#[test]
fn job_workspace_is_not_the_data_dir() {
    let (dir, _store, host, soul) = open_work();
    let job = public_start(&host, soul, "notes");
    let root = crate::workspace_root(dir.path());
    assert_eq!(root, dir.path().join("workspace"));
    assert_ne!(root, dir.path());
    assert!(std::path::Path::new(&job.workspace_dir).starts_with(&root));
}

#[tokio::test]
async fn surface_fs_write_upgrades_without_invoking() {
    let (_dir, store, host, soul) = open_work();
    let registry = ToolRegistry::new();
    let hit = Arc::new(AtomicBool::new(false));
    registry.register_with(
        fs_write_def(),
        Arc::new(SpyInvoke {
            hit: Arc::clone(&hit),
        }),
    );
    registry.register(utility_time_def());
    assert_eq!(
        surface_call_kind(&registry, "fs.write"),
        SurfaceCallKind::Upgrade
    );
    assert_eq!(
        surface_call_kind(&registry, "utility.time"),
        SurfaceCallKind::Run
    );
    let err = registry
        .execute(
            "fs.write",
            json!({"path":"/tmp/x","text":"no"}),
            Layer::Surface,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ene_registry::PipelineError::NotOnSurface(_)));
    let job = host
        .auto_upgrade(UpgradeRequest {
            soul_id: soul,
            goal: "write the file".into(),
            reason: UpgradeReason::SideEffectTool,
            steps_so_far: String::new(),
            brief: Some("so far: (nothing yet). next tool requested: fs.write".into()),
            created_from_turn: None,
        })
        .unwrap();
    assert!(!hit.load(Ordering::SeqCst));
    assert_eq!(job.mode, DelegationMode::Public);
    assert_eq!(store.list_jobs(soul).unwrap().len(), 1);
}

#[tokio::test]
async fn empty_side_effect_tool_runs_on_surface_router() {
    let (_dir, _store, host, soul) = open_work();
    let registry = Arc::new(ToolRegistry::new());
    registry.register(utility_time_def());
    let router = WorkSurfaceRouter::new(host, Arc::clone(&registry), soul, 4);
    let outcome = router.on_tool("utility.time", json!({}), 0).await.unwrap();
    assert!(matches!(outcome, SurfaceToolOutcome::Result(_)));
}

#[tokio::test]
async fn surface_router_upgrades_fs_write_without_spy() {
    let (_dir, store, host, soul) = open_work();
    let registry = Arc::new(ToolRegistry::new());
    let hit = Arc::new(AtomicBool::new(false));
    registry.register_with(
        fs_write_def(),
        Arc::new(SpyInvoke {
            hit: Arc::clone(&hit),
        }),
    );
    let router = WorkSurfaceRouter::new(host, registry, soul, 4);
    let outcome = router
        .on_tool("fs.write", json!({"path":"a","text":"b"}), 0)
        .await
        .unwrap();
    assert!(matches!(outcome, SurfaceToolOutcome::Delegated { .. }));
    assert!(!hit.load(Ordering::SeqCst));
    assert_eq!(store.list_jobs(soul).unwrap().len(), 1);
}

#[tokio::test]
async fn step_budget_upgrades_even_for_empty_side_effects() {
    let (_dir, store, host, soul) = open_work();
    let registry = Arc::new(ToolRegistry::new());
    registry.register(utility_time_def());
    let router = WorkSurfaceRouter::new(host, registry, soul, 2);
    assert!(should_upgrade_steps(1, 2));
    let outcome = router.on_tool("utility.time", json!({}), 1).await.unwrap();
    assert!(matches!(outcome, SurfaceToolOutcome::Delegated { .. }));
    assert_eq!(store.list_jobs(soul).unwrap().len(), 1);
}

#[tokio::test]
async fn lane_prompt_still_works_while_job_running() {
    let dir = TempDir::new().unwrap();
    let work = Arc::new(WorkStore::open(dir.path().join("companions.db")).unwrap());
    let host = Arc::new(DelegationHost::new(
        Arc::clone(&work),
        dir.path().to_path_buf(),
    ));
    let soul = SoulId::new();
    let job = public_start(&host, soul, "research");
    work.set_status(job.id, JobStatus::Running, None).unwrap();
    let sessions = Arc::new(
        SessionStore::open(dir.path().join("sessions.db"), "NORMAL")
            .await
            .unwrap(),
    );
    let session = sessions
        .create_session(NewSession {
            soul_id: soul,
            body_id: None,
            kind: SessionKind::Conversation,
            delegation_id: None,
            created_by: SessionCreatedBy::Client,
        })
        .await
        .unwrap();
    let lane = LaneHandle::spawn(LaneOptions {
        store: Arc::clone(&sessions),
        session,
        soul,
        model: Arc::new(ene_kernel::EchoModel) as Arc<dyn ConversationModel>,
        harness: HarnessSettings::default(),
        mind: LaneMindSettings::default(),
        recovery: Vec::new(),
        speech: None,
        finalizer: None,
        prefetch: None,
        extra_context: Vec::new(),
        hooks: None,
        router: None,
    });
    lane.prompt("hello while working").await.unwrap();
    lane.wait_for_idle().await.unwrap();
    let still = work.get_job(job.id).unwrap().unwrap();
    assert_eq!(still.status, JobStatus::Running);
    let history = lane.project(DisplayDepth::Surface).unwrap();
    assert!(
        history
            .messages
            .iter()
            .any(|m| m.text().contains("hello while working"))
    );
}

#[tokio::test]
async fn lane_auto_upgrade_does_not_execute_fs_write() {
    let dir = TempDir::new().unwrap();
    let work = Arc::new(WorkStore::open(dir.path().join("companions.db")).unwrap());
    let host = Arc::new(DelegationHost::new(
        Arc::clone(&work),
        dir.path().to_path_buf(),
    ));
    let soul = SoulId::new();
    let registry = Arc::new(ToolRegistry::new());
    let hit = Arc::new(AtomicBool::new(false));
    registry.register_with(
        fs_write_def(),
        Arc::new(SpyInvoke {
            hit: Arc::clone(&hit),
        }),
    );
    let router = Arc::new(WorkSurfaceRouter::new(
        Arc::clone(&host),
        Arc::clone(&registry),
        soul,
        4,
    ));
    let sessions = Arc::new(
        SessionStore::open(dir.path().join("sessions.db"), "NORMAL")
            .await
            .unwrap(),
    );
    let session = sessions
        .create_session(NewSession {
            soul_id: soul,
            body_id: None,
            kind: SessionKind::Conversation,
            delegation_id: None,
            created_by: SessionCreatedBy::Client,
        })
        .await
        .unwrap();
    let model = Arc::new(SequenceModel {
        remaining: parking_lot::Mutex::new(vec![ModelGeneration {
            tool_calls: vec![ToolCall {
                name: "fs.write".into(),
                arguments: json!({"path":"x","text":"y"}),
            }],
            ..ModelGeneration::default()
        }]),
    });
    let lane = LaneHandle::spawn(LaneOptions {
        store: sessions,
        session,
        soul,
        model,
        harness: HarnessSettings::default(),
        mind: LaneMindSettings::default(),
        recovery: Vec::new(),
        speech: None,
        finalizer: None,
        prefetch: None,
        extra_context: Vec::new(),
        hooks: None,
        router: Some(router as Arc<dyn SurfaceRouter>),
    });
    lane.prompt("please write a file").await.unwrap();
    lane.wait_for_idle().await.unwrap();
    assert!(!hit.load(Ordering::SeqCst));
    assert_eq!(work.list_jobs(soul).unwrap().len(), 1);
}

#[tokio::test]
async fn drive_job_runs_echo_model_to_completion() {
    let dir = TempDir::new().unwrap();
    let work = Arc::new(WorkStore::open(dir.path().join("companions.db")).unwrap());
    let host = Arc::new(DelegationHost::new(
        Arc::clone(&work),
        dir.path().to_path_buf(),
    ));
    let soul = SoulId::new();
    let job = public_start(&host, soul, "summarize notes");
    let registry = Arc::new(ToolRegistry::new());
    register_work_tools(&registry, Arc::clone(&host), dir.path().join("skills"));
    registry.register(utility_time_def());
    let sessions = Arc::new(
        SessionStore::open(dir.path().join("sessions.db"), "NORMAL")
            .await
            .unwrap(),
    );
    drive_job(JobDrive {
        host: Arc::clone(&host),
        registry,
        sessions,
        model: Arc::new(EchoModel) as Arc<dyn ConversationModel>,
        job: job.clone(),
        step_budget: 8,
        wall: StdDuration::from_secs(10),
    })
    .await
    .unwrap();
    let done = work.get_job(job.id).unwrap().unwrap();
    assert_eq!(done.status, JobStatus::Completed);
    let mail = work.mailbox(job.id).unwrap();
    assert!(
        mail.iter()
            .any(|(dir, kind, _)| dir == "child_to_parent" && kind == "complete"),
        "job lane must send a complete mailbox row, got {mail:?}"
    );
}

#[tokio::test]
async fn drive_job_does_not_revive_cancelled() {
    let dir = TempDir::new().unwrap();
    let work = Arc::new(WorkStore::open(dir.path().join("companions.db")).unwrap());
    let host = Arc::new(DelegationHost::new(
        Arc::clone(&work),
        dir.path().to_path_buf(),
    ));
    let soul = SoulId::new();
    let job = public_start(&host, soul, "cancelled research");
    host.cancel(job.id).unwrap();
    let registry = Arc::new(ToolRegistry::new());
    register_work_tools(&registry, Arc::clone(&host), dir.path().join("skills"));
    let sessions = Arc::new(
        SessionStore::open(dir.path().join("sessions.db"), "NORMAL")
            .await
            .unwrap(),
    );
    drive_job(JobDrive {
        host: Arc::clone(&host),
        registry,
        sessions,
        model: Arc::new(EchoModel) as Arc<dyn ConversationModel>,
        job: job.clone(),
        step_budget: 8,
        wall: StdDuration::from_secs(10),
    })
    .await
    .unwrap();
    let current = work.get_job(job.id).unwrap().unwrap();
    assert_eq!(current.status, JobStatus::Cancelled);
}

#[tokio::test]
async fn job_router_confines_fs_to_job_workspace() {
    let (dir, _store, host, soul) = open_work();
    let job = public_start(&host, soul, "list files");
    let parent = crate::workspace_root(dir.path());
    std::fs::create_dir_all(&parent).unwrap();
    std::fs::write(parent.join("sibling.txt"), "nope").unwrap();
    let registry = Arc::new(ToolRegistry::new());
    registry.set_workspace(&parent);
    let capture = Arc::new(CaptureInvoke {
        last: parking_lot::Mutex::new(None),
    });
    registry.register_with(
        ToolDefinition {
            name: "fs.list".to_owned(),
            description: "List".to_owned(),
            parameters: json!({"type":"object"}),
            output: json!({"type":"object"}),
            side_effects: Vec::new(),
            source: ToolSource::Harness {
                name: "fs".to_owned(),
            },
            timeout_ms: Some(1_000),
            sensitivity: Sensitivity::None,
            category: String::new(),
            keywords: Vec::new(),
            examples: Vec::new(),
            background: false,
        },
        Arc::clone(&capture) as Arc<dyn ToolInvoke>,
    );
    let router = crate::router::JobLayerRouter::new(
        Arc::clone(&host),
        registry,
        soul,
        job.id,
        &job.workspace_dir,
    );
    let err = router
        .on_tool("fs.list", json!({"path": "../sibling.txt"}), 0)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("path escapes") || err.to_string().contains("sibling"),
        "job fs.list must not reach sibling paths, got {err}"
    );
    router.on_tool("fs.list", json!({}), 0).await.unwrap();
    let args = capture.last.lock().clone().unwrap();
    let listed = args["path"].as_str().unwrap();
    assert!(
        listed == job.workspace_dir || listed.starts_with(&job.workspace_dir),
        "empty fs.list must stay in the job workspace, got {listed}"
    );
}

#[tokio::test]
async fn job_briefing_bounds_filesystem_paths_to_the_job_workspace() {
    let (dir, _store, host, soul) = open_work();
    let job = public_start(&host, soul, "write smoke-gui/note.txt");
    let registry = Arc::new(ToolRegistry::new());
    register_work_tools(&registry, Arc::clone(&host), dir.path().join("skills"));
    registry.register(fs_write_def());
    let model = Arc::new(CapturePrompt {
        prompts: parking_lot::Mutex::new(Vec::new()),
        remaining: parking_lot::Mutex::new(vec![ModelGeneration {
            tool_calls: vec![ToolCall {
                name: "delegation.send".into(),
                arguments: json!({"kind":"complete","body":"done"}),
            }],
            ..ModelGeneration::default()
        }]),
    });
    drive_job(JobDrive {
        host: Arc::clone(&host),
        registry,
        sessions: Arc::new(
            SessionStore::open(dir.path().join("sessions.db"), "NORMAL")
                .await
                .unwrap(),
        ),
        model: Arc::clone(&model) as Arc<dyn ConversationModel>,
        job: job.clone(),
        step_budget: 4,
        wall: StdDuration::from_secs(10),
    })
    .await
    .unwrap();
    let prompts = model.prompts.lock().join("\n");
    assert!(
        prompts.contains(job.workspace_dir.trim_end_matches('/'))
            && prompts.contains("Use workspace-relative paths for filesystem tools"),
        "job briefing must provide the scoped workspace and relative-path rule, got {prompts}"
    );
}

#[tokio::test]
async fn drive_job_executes_tools_and_delegation_send() {
    let dir = TempDir::new().unwrap();
    let work = Arc::new(WorkStore::open(dir.path().join("companions.db")).unwrap());
    let host = Arc::new(DelegationHost::new(
        Arc::clone(&work),
        dir.path().to_path_buf(),
    ));
    let soul = SoulId::new();
    let job = public_start(&host, soul, "look up the time");
    let registry = Arc::new(ToolRegistry::new());
    register_work_tools(&registry, Arc::clone(&host), dir.path().join("skills"));
    let audit = ene_plane::AuditLog::open(dir.path().join("audit.db")).unwrap();
    let plane = Arc::new(ene_plane::ApprovalPlane::new(
        ene_plane::ApprovalSettings::default(),
        audit,
        ene_plane::ScriptedPopup::deny_all(),
        None,
    ));
    plane.set_mode(ene_plane::ApprovalMode::Auto).unwrap();
    registry.set_plane(plane);
    let hit = Arc::new(AtomicBool::new(false));
    registry.register_with(
        utility_time_def(),
        Arc::new(CountingInvoke {
            hit: Arc::clone(&hit),
        }),
    );
    let sessions = Arc::new(
        SessionStore::open(dir.path().join("sessions.db"), "NORMAL")
            .await
            .unwrap(),
    );
    let model = Arc::new(SequenceModel {
        remaining: parking_lot::Mutex::new(vec![
            ModelGeneration {
                tool_calls: vec![ToolCall {
                    name: "delegation.send".into(),
                    arguments: json!({"kind":"complete","body":"got the time"}),
                }],
                ..ModelGeneration::default()
            },
            ModelGeneration {
                tool_calls: vec![ToolCall {
                    name: "delegation.send".into(),
                    arguments: json!({"kind":"progress","body":"checking the clock"}),
                }],
                ..ModelGeneration::default()
            },
            ModelGeneration {
                tool_calls: vec![ToolCall {
                    name: "utility.time".into(),
                    arguments: json!({}),
                }],
                ..ModelGeneration::default()
            },
        ]),
    });
    drive_job(JobDrive {
        host: Arc::clone(&host),
        registry,
        sessions,
        model: model as Arc<dyn ConversationModel>,
        job: job.clone(),
        step_budget: 8,
        wall: StdDuration::from_secs(10),
    })
    .await
    .unwrap();
    assert!(
        hit.load(Ordering::SeqCst),
        "job model must run utility.time"
    );
    let done = work.get_job(job.id).unwrap().unwrap();
    assert_eq!(done.status, JobStatus::Completed);
    let mail = work.mailbox(job.id).unwrap();
    assert!(
        mail.iter().any(|(dir, kind, body)| dir == "child_to_parent"
            && kind == "progress"
            && body.contains("clock")),
        "expected progress send, got {mail:?}"
    );
    assert!(
        mail.iter().any(|(dir, kind, body)| dir == "child_to_parent"
            && kind == "complete"
            && body.contains("got the time")),
        "expected complete send, got {mail:?}"
    );
}

#[test]
fn job_persists_and_recover_does_not_resume() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("companions.db");
    let soul = SoulId::new();
    let id;
    {
        let store = Arc::new(WorkStore::open(&path).unwrap());
        let host = DelegationHost::new(Arc::clone(&store), dir.path().to_path_buf());
        let job = public_start(&host, soul, "long task");
        store.set_status(job.id, JobStatus::Running, None).unwrap();
        id = job.id;
    }
    let store = Arc::new(WorkStore::open(&path).unwrap());
    let host = DelegationHost::new(Arc::clone(&store), dir.path().to_path_buf());
    let reports = host.recover_interrupted().unwrap();
    assert_eq!(reports.len(), 1);
    assert!(reports[0].speech.contains("long task"));
    assert!(reports[0].speech.contains("stopped"));
    let job = store.get_job(id).unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Interrupted);
    assert!(host.recover_interrupted().unwrap().is_empty());
}

#[test]
fn progress_and_complete_are_companion_speech() {
    let (_dir, _store, host, soul) = open_work();
    let job = public_start(&host, soul, "draft");
    let progress = host.progress(job.id, Some(0.4), "outlining").unwrap();
    assert!(progress.speech.contains("outlining"));
    assert!(!progress.starts_conversation);
    let done = host.complete(job.id, "the outline is ready").unwrap();
    assert!(done.speech.contains("the outline is ready"));
    assert!(done.starts_conversation);
    assert!(matches!(
        host.cancel(job.id),
        Err(crate::WorkError::AlreadyCompleted)
    ));
}

#[test]
fn cancel_of_cancelled_is_idempotent_error() {
    let (_dir, _store, host, soul) = open_work();
    let job = public_start(&host, soul, "x");
    assert_eq!(host.cancel(job.id).unwrap(), JobStatus::Cancelled);
    assert!(matches!(
        host.cancel(job.id),
        Err(crate::WorkError::Cancelled)
    ));
}

#[test]
fn internal_delegation_has_no_job_row() {
    let (_dir, store, host, soul) = open_work();
    let job = host
        .start(StartDelegation {
            soul_id: soul,
            goal: "secret lookup".into(),
            mode: DelegationMode::Internal,
            title: None,
            brief: None,
            plan: None,
            created_from_turn: None,
            depth: 0,
            parent_id: None,
        })
        .unwrap();
    assert!(store.get_job(job.id).unwrap().is_none());
    assert!(store.list_jobs(soul).unwrap().is_empty());
    assert!(!store.mailbox(job.id).unwrap().is_empty());
}

#[test]
fn schedule_remind_fires_and_quiet_hours_defer() {
    let (_dir, store, _host, soul) = open_work();
    let now = Utc.with_ymd_and_hms(2026, 8, 17, 3, 0, 0).unwrap();
    let remind = store
        .insert_schedule(&NewSchedule {
            soul_id: soul,
            name: "tea".into(),
            spec: "* * * * * *".into(),
            timezone: "UTC".into(),
            action: ScheduleAction::Remind,
            action_ref: Some("drink tea".into()),
            important: false,
        })
        .unwrap();
    store
        .defer_next_fire(&remind.id, now - Duration::minutes(1))
        .unwrap();
    let quiet = QuietWindow {
        enabled: true,
        start_hour: 0,
        end_hour: 24,
        timezone: "UTC".into(),
    };
    assert!(fire_due(&store, now, &quiet).unwrap().is_empty());
    let important = store
        .insert_schedule(&NewSchedule {
            soul_id: soul,
            name: "meds".into(),
            spec: "* * * * * *".into(),
            timezone: "UTC".into(),
            action: ScheduleAction::Remind,
            action_ref: Some("take meds".into()),
            important: true,
        })
        .unwrap();
    store
        .defer_next_fire(&important.id, now - Duration::minutes(1))
        .unwrap();
    let fired = fire_due(&store, now, &quiet).unwrap();
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].schedule.name, "meds");
    assert!(
        reminder_report(&fired[0].schedule)
            .speech
            .contains("take meds")
    );
}

#[test]
fn missed_job_schedule_does_not_run() {
    let (_dir, store, _host, soul) = open_work();
    let now = Utc.with_ymd_and_hms(2026, 8, 17, 12, 0, 0).unwrap();
    let job_sched = store
        .insert_schedule(&NewSchedule {
            soul_id: soul,
            name: "nightly".into(),
            spec: "0 0 * * * *".into(),
            timezone: "UTC".into(),
            action: ScheduleAction::Job,
            action_ref: None,
            important: false,
        })
        .unwrap();
    let past = now - Duration::hours(2);
    store.defer_next_fire(&job_sched.id, past).unwrap();
    assert!(catch_up_missed(&store, now).unwrap().is_empty());
    let updated = store.get_schedule(&job_sched.id).unwrap().unwrap();
    assert_ne!(
        updated.next_fire.as_deref(),
        Some(past.to_rfc3339().as_str())
    );
}

#[test]
fn missed_remind_fires_once() {
    let (_dir, store, _host, soul) = open_work();
    let now = Utc.with_ymd_and_hms(2026, 8, 17, 12, 0, 0).unwrap();
    let remind = store
        .insert_schedule(&NewSchedule {
            soul_id: soul,
            name: "call".into(),
            spec: "* * * * * *".into(),
            timezone: "UTC".into(),
            action: ScheduleAction::Remind,
            action_ref: Some("call mom".into()),
            important: false,
        })
        .unwrap();
    store
        .defer_next_fire(&remind.id, now - Duration::minutes(5))
        .unwrap();
    let fired = catch_up_missed(&store, now).unwrap();
    assert_eq!(fired.len(), 1);
    assert!(fired[0].missed);
}

#[test]
fn skill_install_and_load() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("SKILL.md"),
        "---\nname: travel\ndescription: plan a trip\n---\n\n# Travel\npack light\n",
    )
    .unwrap();
    let home = dir.path().join("skills");
    let installed = install_skill_dir(&home, &src).unwrap();
    assert_eq!(installed.meta.name, "travel");
    let loaded = load_skill(&home, "travel").unwrap();
    assert!(loaded.body.contains("pack light"));
    assert_eq!(catalog(&home, &["travel".into()]).unwrap().len(), 1);
    assert!(matches!(
        load_skill(&home, "missing"),
        Err(crate::WorkError::UnknownSkill(_))
    ));
    assert!(parse_skill_md("not a skill").is_err());
}

#[test]
fn matching_skill_enters_catalog_and_active_context() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(src.join("references")).unwrap();
    std::fs::write(
        src.join("SKILL.md"),
        "---\nname: travel\ndescription: 旅行の計画・しおり作成を支援する\n---\n\n# Travel\npack light\n",
    )
    .unwrap();
    std::fs::write(src.join("references/checklist.md"), "- passport\n").unwrap();
    let home = dir.path().join("skills");
    install_skill_dir(&home, &src).unwrap();
    let catalog = skill_catalog_blocks(&home, &[]);
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].0, "skills.catalog");
    assert!(catalog[0].1.contains("travel"));
    let matched = match_skills(&home, &[], "東京を調べてしおりにまとめて").unwrap();
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].name, "travel");
    let active = skill_active_blocks(&home, &[], "東京を調べてしおりにまとめて");
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].0, "skills.active");
    assert!(active[0].1.contains("pack light"));
    let hash = match_skills(&home, &[], "hash this file").unwrap();
    assert!(hash.is_empty());
    assert!(skill_active_blocks(&home, &[], "hash this file").is_empty());
    let checklist = read_skill_file(&home, "travel", "references/checklist.md").unwrap();
    assert!(checklist.contains("passport"));
    assert!(read_skill_file(&home, "travel", "../escape.md").is_err());
    assert!(read_skill_file(&home, "/etc", "passwd").is_err());
    assert!(read_skill_file(&home, "..", "SKILL.md").is_err());
}

#[test]
fn ene_frontmatter_feeds_proactive_and_emotion_helpers() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("SKILL.md"),
        "---\nname: travel\ndescription: 旅行の計画・しおり作成を支援する\nene.proactive_hint: Offer a morning briefing\nene.emotion_note: keep it light\n---\n\n# Travel\npack light\n",
    )
    .unwrap();
    let home = dir.path().join("skills");
    install_skill_dir(&home, &src).unwrap();
    let loaded = load_skill(&home, "travel").unwrap();
    assert_eq!(
        loaded.proactive_hint.as_deref(),
        Some("Offer a morning briefing")
    );
    assert_eq!(loaded.emotion_note.as_deref(), Some("keep it light"));
    assert_eq!(
        skill_proactive_hints(&home, &[]),
        vec!["Offer a morning briefing"]
    );
    assert_eq!(
        skill_emotion_notes(&home, &[], "東京を調べてしおりにまとめて"),
        vec!["keep it light"]
    );
    assert!(skill_emotion_notes(&home, &[], "hash this file").is_empty());
    let active = skill_active_blocks(&home, &[], "東京を調べてしおりにまとめて");
    assert!(active[0].1.contains("Tone: keep it light"));
    assert!(active[0].1.contains("pack light"));
    assert!(skill_proactive_hints(&home, &["other".into()]).is_empty());
}

#[tokio::test]
async fn bookmark_job_searches_and_applies_matching_skill() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(WorkStore::open(dir.path().join("companions.db")).unwrap());
    let host = Arc::new(DelegationHost::new(
        Arc::clone(&store),
        dir.path().to_path_buf(),
    ));
    let soul = SoulId::new();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("SKILL.md"),
        "---\nname: travel\ndescription: 旅行の計画・しおり作成を支援する\n---\n\nWrite an itinerary bookmark.\n",
    )
    .unwrap();
    let home = dir.path().join("skills");
    install_skill_dir(&home, &src).unwrap();
    let job = public_start(&host, soul, "東京を調べてしおりにまとめて");
    host.present_plan(job.id, "1. search\n2. write bookmark")
        .unwrap();
    host.approve_plan(job.id).unwrap();
    let registry = Arc::new(ToolRegistry::new());
    registry.register_with(
        ToolDefinition {
            name: "web.search".to_owned(),
            description: "search".to_owned(),
            parameters: json!({"type":"object"}),
            output: json!({"type":"object"}),
            side_effects: Vec::new(),
            source: ToolSource::Harness {
                name: "web".to_owned(),
            },
            timeout_ms: Some(1_000),
            sensitivity: Sensitivity::None,
            category: String::new(),
            keywords: Vec::new(),
            examples: Vec::new(),
            background: false,
        },
        Arc::new(SearchInvoke) as Arc<dyn ToolInvoke>,
    );
    let (artifact, report) = crate::fill_bookmark_job(crate::BookmarkFill {
        host: host.as_ref(),
        soul_id: soul,
        job_id: job.id,
        theme: "東京を調べてしおりにまとめて",
        skills_home: &home,
        enabled: &[],
        registry: Some(registry.as_ref()),
    })
    .await
    .unwrap();
    assert!(artifact.delivered);
    assert!(report.speech.contains("bookmark ready"));
    let content = std::fs::read_to_string(&artifact.path).unwrap();
    assert!(content.contains("Shibuya crossing"));
    assert!(content.contains("Write an itinerary bookmark"));
    assert!(content.contains("https://example.invalid/tokyo"));
}

#[tokio::test]
async fn mcp_handwritten_tools_execute_through_registry() {
    let registry = ToolRegistry::new();
    let invoke: Arc<dyn ToolInvoke> = Arc::new(ScriptedMcp::new([(
        "mcp:git.status".into(),
        json!({"clean": true}),
    )]));
    register_mcp_tools(
        &registry,
        &McpProfile {
            server: "git".into(),
            transport: "stdio".into(),
            command: Some("git-mcp".into()),
            url: None,
        },
        vec![McpTool {
            name: "status".into(),
            description: "git status".into(),
            parameters: json!({"type":"object"}),
            side_effects: Vec::new(),
        }],
        &invoke,
    );
    let value = registry
        .execute("mcp:git.status", json!({}), Layer::Job)
        .await
        .unwrap();
    assert_eq!(value["clean"], json!(true));
    assert!(!registry.get("mcp:git.status").unwrap().background);
}

#[test]
fn screenshot_is_surface_and_high_sensitivity() {
    let registry = ToolRegistry::new();
    register_screenshot_tool(&registry, Arc::new(PngScreenshot::minimal()));
    assert!(screenshot_is_job_or_surface(&registry));
    let def = registry.get("app.screenshot").unwrap();
    assert!(def.side_effects.is_empty());
    assert_eq!(def.sensitivity, Sensitivity::High);
}

#[tokio::test]
async fn tool_path_returns_png_and_placeholder_is_unavailable() {
    let registry = ToolRegistry::new();
    register_screenshot_tool(&registry, Arc::new(PngScreenshot::minimal()));
    let value = registry
        .execute("app.screenshot", json!({}), Layer::Surface)
        .await
        .unwrap();
    let png = screenshot_png(&value).unwrap();
    assert_eq!(png, crate::vision::MINIMAL_PNG);
    let empty = ToolRegistry::new();
    register_screenshot_tool(&empty, Arc::new(PlaceholderScreenshot));
    let err = capture_screenshot(&empty).await.unwrap_err();
    assert_eq!(err, ScreenshotError::Unavailable);
    assert_eq!(
        screenshot_png(&json!({"available": false})).unwrap_err(),
        ScreenshotError::Unavailable
    );
}

#[tokio::test]
async fn observe_screen_from_png_does_not_enter_session_history() {
    let dir = TempDir::new().unwrap();
    let sessions = SessionStore::open(dir.path().join("sessions.db"), "NORMAL")
        .await
        .unwrap();
    let soul = SoulId::new();
    let session = sessions
        .create_session(NewSession {
            soul_id: soul,
            body_id: None,
            kind: SessionKind::Conversation,
            delegation_id: None,
            created_by: SessionCreatedBy::Client,
        })
        .await
        .unwrap();
    let registry = ToolRegistry::new();
    register_screenshot_tool(&registry, Arc::new(PngScreenshot::minimal()));
    let png = capture_screenshot(&registry).await.unwrap();
    assert!(png.starts_with(b"\x89PNG"));
    let summary = "terminal with cargo test";
    let mut memory = WorldStateMemory::default();
    let snap = observe_screen(
        &mut memory,
        &WorldStateSettings {
            enabled: true,
            ..WorldStateSettings::default()
        },
        summary,
        12,
    );
    assert!(!format!("{snap:?}").contains(summary));
    let events = sessions.load_events(session, 0).unwrap();
    let history = derive_messages(&events, ene_session::ProjectOptions::model_visible(8));
    let text = history
        .messages
        .iter()
        .map(ene_session::ProjectedMessage::text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains(summary));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, EventKind::UserMessage))
    );
    let snap_json = serde_json::to_vec(&snap).unwrap();
    assert!(!contains_raw_screenshot(&snap_json));
    assert!(!contains_raw_screenshot(format!("{memory:?}").as_bytes()));
}

#[test]
fn observation_gate_does_not_keep_png_in_pipeline() {
    let mut pipe = ObservationPipeline::new();
    let png = crate::vision::MINIMAL_PNG.to_vec();
    let first = pipe.evaluate(&png).unwrap();
    assert!(matches!(first, ObserveAction::Changed { .. }));
    pipe.commit_summary("one pixel".to_owned());
    let second = pipe.evaluate(&png).unwrap();
    assert!(matches!(second, ObserveAction::Skip { .. }));
    assert!(!contains_raw_screenshot(format!("{pipe:?}").as_bytes()));
}

#[tokio::test]
async fn artifact_register_and_deliver() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(WorkStore::open(dir.path().join("companions.db")).unwrap());
    let host = Arc::new(DelegationHost::new(
        Arc::clone(&store),
        dir.path().to_path_buf(),
    ));
    let soul = SoulId::new();
    let job = public_start(&host, soul, "notes");
    host.present_plan(job.id, "1. write notes\n2. register artifact")
        .unwrap();
    host.approve_plan(job.id).unwrap();
    let registry = Arc::new(ToolRegistry::new());
    register_work_tools(&registry, Arc::clone(&host), dir.path().join("skills"));
    let audit = ene_plane::AuditLog::open(dir.path().join("audit.db")).unwrap();
    let plane = Arc::new(ene_plane::ApprovalPlane::new(
        ene_plane::ApprovalSettings::default(),
        audit,
        ene_plane::ScriptedPopup::deny_all(),
        None,
    ));
    plane.set_policy(ene_plane::PolicyFile {
        rules: vec![ene_plane::PolicyRule {
            tool: "artifact.register".to_owned(),
            scope: None,
            decision: ene_plane::PolicyDecision::Allow,
        }],
    });
    registry.set_plane(plane);
    let workspace = crate::workspace_root(dir.path());
    std::fs::create_dir_all(&workspace).unwrap();
    registry.set_workspace(&workspace);
    assert!(surface_shows_delegate(&registry));
    let file = workspace.join("out.md");
    std::fs::write(&file, "# hi").unwrap();
    let registered = registry
        .execute(
            "artifact.register",
            json!({
                "soul_id": soul.to_string(),
                "job_id": job.id.to_string(),
                "kind": "markdown",
                "title": "notes",
                "path": file.to_string_lossy(),
            }),
            Layer::Job,
        )
        .await
        .unwrap();
    let art_id = registered["id"].as_str().unwrap();
    host.complete(job.id, "notes ready").unwrap();
    let arts = store.artifacts_for(job.id).unwrap();
    assert_eq!(arts.len(), 1);
    assert_eq!(arts[0].id, art_id);
    assert!(arts[0].delivered);
    let dest = crate::soul_artifacts_dir(dir.path(), soul);
    assert!(
        std::path::Path::new(&arts[0].path).starts_with(&dest),
        "delivered path should be under soul artifacts dir, got {}",
        arts[0].path
    );
    assert_eq!(std::fs::read_to_string(&arts[0].path).unwrap(), "# hi");
    assert!(ArtifactKind::try_parse("docx").is_err());
}

#[tokio::test]
async fn artifact_register_rejects_path_outside_workspace() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(WorkStore::open(dir.path().join("companions.db")).unwrap());
    let host = Arc::new(DelegationHost::new(
        Arc::clone(&store),
        dir.path().to_path_buf(),
    ));
    let soul = SoulId::new();
    let job = public_start(&host, soul, "notes");
    host.present_plan(job.id, "1. write notes").unwrap();
    host.approve_plan(job.id).unwrap();
    let registry = Arc::new(ToolRegistry::new());
    register_work_tools(&registry, Arc::clone(&host), dir.path().join("skills"));
    let audit = ene_plane::AuditLog::open(dir.path().join("audit.db")).unwrap();
    let plane = Arc::new(ene_plane::ApprovalPlane::new(
        ene_plane::ApprovalSettings::default(),
        audit,
        ene_plane::ScriptedPopup::deny_all(),
        None,
    ));
    plane.set_policy(ene_plane::PolicyFile {
        rules: vec![ene_plane::PolicyRule {
            tool: "artifact.register".to_owned(),
            scope: None,
            decision: ene_plane::PolicyDecision::Allow,
        }],
    });
    registry.set_plane(plane);
    let outside = dir.path().join("secret.txt");
    std::fs::write(&outside, "token").unwrap();
    let err = registry
        .execute(
            "artifact.register",
            json!({
                "soul_id": soul.to_string(),
                "job_id": job.id.to_string(),
                "kind": "markdown",
                "title": "leak",
                "path": outside.to_string_lossy(),
            }),
            Layer::Job,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("escapes workspace"));
}

#[tokio::test]
async fn artifact_register_requires_job_id() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(WorkStore::open(dir.path().join("companions.db")).unwrap());
    let host = Arc::new(DelegationHost::new(
        Arc::clone(&store),
        dir.path().to_path_buf(),
    ));
    let soul = SoulId::new();
    let registry = Arc::new(ToolRegistry::new());
    register_work_tools(&registry, host, dir.path().join("skills"));
    let audit = ene_plane::AuditLog::open(dir.path().join("audit.db")).unwrap();
    let plane = Arc::new(ene_plane::ApprovalPlane::new(
        ene_plane::ApprovalSettings::default(),
        audit,
        ene_plane::ScriptedPopup::deny_all(),
        None,
    ));
    plane.set_policy(ene_plane::PolicyFile {
        rules: vec![ene_plane::PolicyRule {
            tool: "artifact.register".to_owned(),
            scope: None,
            decision: ene_plane::PolicyDecision::Allow,
        }],
    });
    registry.set_plane(plane);
    let workspace = crate::workspace_root(dir.path());
    std::fs::create_dir_all(&workspace).unwrap();
    let file = workspace.join("out.md");
    std::fs::write(&file, "# hi").unwrap();
    let err = registry
        .execute(
            "artifact.register",
            json!({
                "soul_id": soul.to_string(),
                "kind": "markdown",
                "title": "notes",
                "path": file.to_string_lossy(),
            }),
            Layer::Job,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("missing job_id"));
}

#[test]
fn complete_delivers_all_job_artifacts() {
    let (dir, store, host, soul) = open_work();
    let job = public_start(&host, soul, "pack");
    let workspace = PathBuf::from(&job.workspace_dir);
    std::fs::write(workspace.join("a.md"), "one").unwrap();
    std::fs::write(workspace.join("b.csv"), "x,y").unwrap();
    store
        .register_artifact(Artifact {
            id: "art-a".into(),
            soul_id: soul,
            job_id: Some(job.id),
            kind: ArtifactKind::Markdown,
            title: "alpha".into(),
            path: workspace.join("a.md").to_string_lossy().into_owned(),
            mime: None,
            size_bytes: Some(3),
            created_at: Utc::now().to_rfc3339(),
            delivered: false,
        })
        .unwrap();
    store
        .register_artifact(Artifact {
            id: "art-b".into(),
            soul_id: soul,
            job_id: Some(job.id),
            kind: ArtifactKind::Csv,
            title: "beta".into(),
            path: workspace.join("b.csv").to_string_lossy().into_owned(),
            mime: None,
            size_bytes: Some(3),
            created_at: Utc::now().to_rfc3339(),
            delivered: false,
        })
        .unwrap();
    host.complete(job.id, "packed").unwrap();
    let arts = store.artifacts_for(job.id).unwrap();
    assert_eq!(arts.len(), 2);
    assert!(arts.iter().all(|art| art.delivered));
    let dest = crate::soul_artifacts_dir(dir.path(), soul);
    for art in &arts {
        assert!(
            PathBuf::from(&art.path).starts_with(&dest),
            "expected {} under {}",
            art.path,
            dest.display()
        );
        assert!(PathBuf::from(&art.path).is_file());
    }
}

#[test]
fn fail_does_not_deliver_artifacts() {
    let (_dir, store, host, soul) = open_work();
    let job = public_start(&host, soul, "pack");
    let file = PathBuf::from(&job.workspace_dir).join("draft.md");
    std::fs::write(&file, "nope").unwrap();
    store
        .register_artifact(Artifact {
            id: "art-fail".into(),
            soul_id: soul,
            job_id: Some(job.id),
            kind: ArtifactKind::Markdown,
            title: "draft".into(),
            path: file.to_string_lossy().into_owned(),
            mime: None,
            size_bytes: Some(4),
            created_at: Utc::now().to_rfc3339(),
            delivered: false,
        })
        .unwrap();
    host.fail(job.id, "gave up").unwrap();
    let arts = store.artifacts_for(job.id).unwrap();
    assert_eq!(arts.len(), 1);
    assert!(!arts[0].delivered);
}

#[test]
fn complete_without_artifacts_is_ok() {
    let (_dir, store, host, soul) = open_work();
    let job = public_start(&host, soul, "talk");
    host.complete(job.id, "nothing to hand over").unwrap();
    assert!(store.artifacts_for(job.id).unwrap().is_empty());
}

#[test]
fn complete_fails_when_artifact_source_is_missing() {
    let (_dir, store, host, soul) = open_work();
    let job = public_start(&host, soul, "pack");
    let missing = PathBuf::from(&job.workspace_dir).join("gone.md");
    store
        .register_artifact(Artifact {
            id: "art-missing".into(),
            soul_id: soul,
            job_id: Some(job.id),
            kind: ArtifactKind::Markdown,
            title: "gone".into(),
            path: missing.to_string_lossy().into_owned(),
            mime: None,
            size_bytes: Some(0),
            created_at: Utc::now().to_rfc3339(),
            delivered: false,
        })
        .unwrap();
    assert!(host.complete(job.id, "packed").is_err());
    let arts = store.artifacts_for(job.id).unwrap();
    assert_eq!(arts.len(), 1);
    assert!(!arts[0].delivered);
    let status = store.get_job(job.id).unwrap().unwrap().status;
    assert_ne!(status, JobStatus::Completed);
}

#[test]
fn complete_fails_when_artifact_copy_errors() {
    let (dir, store, host, soul) = open_work();
    let job = public_start(&host, soul, "pack");
    let src = PathBuf::from(&job.workspace_dir).join("draft.md");
    std::fs::write(&src, "body").unwrap();
    store
        .register_artifact(Artifact {
            id: "art-copy".into(),
            soul_id: soul,
            job_id: Some(job.id),
            kind: ArtifactKind::Markdown,
            title: "draft".into(),
            path: src.to_string_lossy().into_owned(),
            mime: None,
            size_bytes: Some(4),
            created_at: Utc::now().to_rfc3339(),
            delivered: false,
        })
        .unwrap();
    let dest = crate::soul_artifacts_dir(dir.path(), soul).join("art-copy_draft.md");
    std::fs::create_dir_all(&dest).unwrap();
    assert!(host.complete(job.id, "packed").is_err());
    let arts = store.artifacts_for(job.id).unwrap();
    assert_eq!(arts.len(), 1);
    assert!(!arts[0].delivered);
    let status = store.get_job(job.id).unwrap().unwrap().status;
    assert_ne!(status, JobStatus::Completed);
}

#[test]
fn question_timeout_is_24h() {
    let asked = Utc.with_ymd_and_hms(2026, 8, 16, 12, 0, 0).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 17, 12, 0, 1).unwrap();
    assert!(question_timed_out(asked, now, StdDuration::from_hours(24)));
    assert!(!question_timed_out(
        asked,
        Utc.with_ymd_and_hms(2026, 8, 17, 11, 0, 0).unwrap(),
        StdDuration::from_hours(24)
    ));
}

#[test]
fn work_tools_cover_delegate_surface() {
    let (_dir, _store, host, _soul) = open_work();
    let registry = Arc::new(ToolRegistry::new());
    register_work_tools(&registry, host, PathBuf::from("/tmp/skills"));
    let names: Vec<String> = registry
        .schemas(Layer::Surface)
        .iter()
        .filter_map(|schema| {
            schema
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert!(names.iter().any(|n| n == "delegate.start"));
    assert!(names.iter().any(|n| n == "delegate.approve_plan"));
    assert!(names.iter().any(|n| n == "skill.load"));
    assert!(names.iter().any(|n| n == "skill.read"));
    assert!(!names.iter().any(|n| n == "delegation.send"));
    assert!(!names.iter().any(|n| n == "artifact.register"));
    assert!(!names.iter().any(|n| n == "workflow.bookmark"));
    let job_names: Vec<String> = registry
        .schemas(Layer::Job)
        .iter()
        .filter_map(|schema| {
            schema
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert!(job_names.iter().any(|n| n == "workflow.bookmark"));
}

#[test]
fn grandchild_delegation_respects_depth_guard() {
    let (_dir, _store, host, soul) = open_work();
    let parent = public_start(&host, soul, "parent task");
    let child = host
        .start(StartDelegation {
            soul_id: soul,
            goal: "child work".into(),
            mode: DelegationMode::Public,
            title: Some("child".into()),
            brief: None,
            plan: None,
            created_from_turn: None,
            depth: 1,
            parent_id: Some(parent.id),
        })
        .unwrap();
    let grandchild = host
        .start(StartDelegation {
            soul_id: soul,
            goal: "grandchild work".into(),
            mode: DelegationMode::Public,
            title: Some("grandchild".into()),
            brief: None,
            plan: None,
            created_from_turn: None,
            depth: 2,
            parent_id: Some(child.id),
        })
        .unwrap();
    assert_eq!(grandchild.goal, "grandchild work");
    assert!(matches!(
        host.start(StartDelegation {
            soul_id: soul,
            goal: "too deep".into(),
            mode: DelegationMode::Public,
            title: None,
            brief: None,
            plan: None,
            created_from_turn: None,
            depth: 3,
            parent_id: Some(grandchild.id),
        }),
        Err(crate::WorkError::DepthExceeded)
    ));
}

#[test]
fn internal_child_cannot_spawn_public_grandchild() {
    let (_dir, _store, host, soul) = open_work();
    let internal = host
        .start(StartDelegation {
            soul_id: soul,
            goal: "secret parent".into(),
            mode: DelegationMode::Internal,
            title: None,
            brief: None,
            plan: None,
            created_from_turn: None,
            depth: 0,
            parent_id: None,
        })
        .unwrap();
    assert!(matches!(
        host.start(StartDelegation {
            soul_id: soul,
            goal: "leak".into(),
            mode: DelegationMode::Public,
            title: None,
            brief: None,
            plan: None,
            created_from_turn: None,
            depth: 1,
            parent_id: Some(internal.id),
        }),
        Err(crate::WorkError::SecrecyViolation)
    ));
    let grandchild = host
        .start(StartDelegation {
            soul_id: soul,
            goal: "still secret".into(),
            mode: DelegationMode::Internal,
            title: None,
            brief: None,
            plan: None,
            created_from_turn: None,
            depth: 1,
            parent_id: Some(internal.id),
        })
        .unwrap();
    assert_eq!(grandchild.mode, DelegationMode::Internal);
}

#[test]
fn combined_child_questions_merge_and_route_answers() {
    let (_dir, _store, host, soul) = open_work();
    let job = public_start(&host, soul, "research");
    host.question(job.id, "which city?").unwrap();
    host.question(job.id, "how many days?").unwrap();
    let combined = host.combine_pending_questions(job.id).unwrap();
    assert!(combined.speech.contains("which city?"));
    assert!(combined.speech.contains("how many days?"));
    assert_eq!(combined.questions.len(), 2);
    host.apply_combined_answers(&combined, &["Tokyo".into(), "3".into()])
        .unwrap();
    assert!(host.open_questions(job.id).unwrap().is_empty());
    let mailbox = host.store().mailbox(job.id).unwrap();
    assert!(
        mailbox
            .iter()
            .any(|(_, kind, body)| kind == "answer" && body == "Tokyo")
    );
    assert!(
        mailbox
            .iter()
            .any(|(_, kind, body)| kind == "answer" && body == "3")
    );
}

#[test]
fn question_report_carries_job_id() {
    let (_dir, _store, host, soul) = open_work();
    let job = public_start(&host, soul, "research");
    let report = host.question(job.id, "which city?").unwrap();
    assert_eq!(report.job_id, Some(job.id));
    assert_eq!(report.inner_intent.as_deref(), Some("ask_user"));
    assert_eq!(report.speech, "which city?");
}

#[test]
fn question_timeout_proceeds_with_assumption() {
    let (_dir, store, host, soul) = open_work();
    let job = public_start(&host, soul, "planning");
    store.set_status(job.id, JobStatus::Running, None).unwrap();
    let asked = Utc.with_ymd_and_hms(2026, 8, 16, 12, 0, 0).unwrap();
    store
        .mailbox_push_at(
            job.id,
            "child_to_parent",
            "question",
            "which airline?",
            &asked.to_rfc3339(),
        )
        .unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 17, 12, 0, 1).unwrap();
    let reports = host
        .resolve_question_timeouts(now, Some(StdDuration::from_hours(24)))
        .unwrap();
    assert_eq!(reports.len(), 1);
    assert!(reports[0].speech.contains("timeout"));
    assert!(!reports[0].starts_conversation);
    let mailbox = store.mailbox(job.id).unwrap();
    assert!(mailbox.iter().any(|(_, kind, _)| kind == "assumption"));
}

#[test]
fn spill_huge_tool_output_keeps_brief_bounded() {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let huge = "x".repeat(10_000);
    let spilled = crate::spill_tool_output(
        &huge,
        &workspace,
        crate::DEFAULT_SOFT_LIMIT_BYTES,
        crate::DEFAULT_HARD_LIMIT_BYTES,
    )
    .unwrap();
    assert!(spilled.spilled);
    assert!(spilled.inline.len() < huge.len());
    assert!(spilled.spill_path.is_some());
    let bounded = crate::bound_brief(&huge, &workspace, 500).unwrap();
    assert!(bounded.len() < huge.len());
}

#[test]
fn bookmark_workflow_delivers_markdown_artifact() {
    let (dir, store, host, soul) = open_work();
    let job = public_start(&host, soul, "travel notes");
    store.set_status(job.id, JobStatus::Running, None).unwrap();
    host.present_plan(job.id, "1. write markdown\n2. deliver")
        .unwrap();
    host.approve_plan(job.id).unwrap();
    let (artifact, report) = crate::deliver_bookmark_workflow(
        &host,
        soul,
        job.id,
        "Tokyo trip",
        &[crate::BookmarkSection {
            heading: "Highlights".into(),
            body: "Shibuya crossing".into(),
        }],
    )
    .unwrap();
    assert_eq!(artifact.kind, ArtifactKind::Markdown);
    assert!(artifact.delivered);
    assert!(report.speech.contains("bookmark ready"));
    let arts = store.artifacts_for(job.id).unwrap();
    assert_eq!(arts.len(), 1);
    let dest = crate::soul_artifacts_dir(dir.path(), soul);
    assert!(PathBuf::from(&artifact.path).starts_with(&dest));
    let content = std::fs::read_to_string(&artifact.path).unwrap();
    assert!(content.contains("# Tokyo trip"));
    assert!(content.contains("Shibuya crossing"));
}

#[test]
fn deliver_bookmark_workflow_requires_plan_approval() {
    let (_dir, store, host, soul) = open_work();
    let job = public_start(&host, soul, "report");
    store.set_status(job.id, JobStatus::Running, None).unwrap();
    assert!(matches!(
        crate::deliver_bookmark_workflow(
            &host,
            soul,
            job.id,
            "Draft",
            &[crate::BookmarkSection {
                heading: "Body".into(),
                body: "content".into(),
            }],
        ),
        Err(crate::WorkError::PlanNotApproved)
    ));
    host.present_plan(job.id, "1. write\n2. ship").unwrap();
    assert!(matches!(
        crate::deliver_bookmark_workflow(
            &host,
            soul,
            job.id,
            "Draft",
            &[crate::BookmarkSection {
                heading: "Body".into(),
                body: "content".into(),
            }],
        ),
        Err(crate::WorkError::PlanNotApproved)
    ));
    host.approve_plan(job.id).unwrap();
    crate::deliver_bookmark_workflow(
        &host,
        soul,
        job.id,
        "Draft",
        &[crate::BookmarkSection {
            heading: "Body".into(),
            body: "content".into(),
        }],
    )
    .unwrap();
}

#[tokio::test]
async fn fill_bookmark_job_requires_plan_before_running() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(WorkStore::open(dir.path().join("companions.db")).unwrap());
    let host = Arc::new(DelegationHost::new(
        Arc::clone(&store),
        dir.path().to_path_buf(),
    ));
    let soul = SoulId::new();
    let job = public_start(&host, soul, "write a bookmark");
    let err = crate::fill_bookmark_job(crate::BookmarkFill {
        host: host.as_ref(),
        soul_id: soul,
        job_id: job.id,
        theme: "tokyo",
        skills_home: &dir.path().join("skills"),
        enabled: &[],
        registry: None,
    })
    .await
    .unwrap_err();
    assert!(matches!(err, crate::WorkError::PlanNotApproved));
    let current = store.get_job(job.id).unwrap().unwrap();
    assert_eq!(current.status, JobStatus::Queued);
}

#[test]
fn surface_message_and_cancel_while_running() {
    let (_dir, store, host, soul) = open_work();
    let job = public_start(&host, soul, "long task");
    store.set_status(job.id, JobStatus::Running, None).unwrap();
    host.message(job.id, "user added context").unwrap();
    host.instruct(job.id, "please prioritize speed").unwrap();
    assert_eq!(host.cancel(job.id).unwrap(), JobStatus::Cancelled);
    let mailbox = store.mailbox(job.id).unwrap();
    assert!(mailbox.iter().any(|(_, kind, _)| kind == "message"));
    assert!(mailbox.iter().any(|(_, kind, _)| kind == "task"));
}

#[test]
fn completion_waits_for_user_speech_gap() {
    let (_dir, _store, host, soul) = open_work();
    drop(host.mark_user_speaking(true));
    let job = public_start(&host, soul, "report");
    let queued = host.complete(job.id, "all findings collected").unwrap();
    assert!(!queued.starts_conversation);
    assert_eq!(queued.inner_intent.as_deref(), Some("complete_queued"));
    let drained = host.mark_user_speaking(false);
    assert_eq!(drained.len(), 1);
    assert!(drained[0].speech.contains("all findings collected"));
}

#[tokio::test]
async fn complete_publishes_when_user_is_not_speaking() {
    let (_dir, _store, host, soul) = open_work();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    host.set_report_sink(tx);
    let job = public_start(&host, soul, "report");
    let report = host.complete(job.id, "all findings collected").unwrap();
    assert!(report.speech.contains("all findings collected"));
    let published = rx.recv().await.unwrap();
    assert_eq!(published.soul_id, soul);
    assert!(published.speech.contains("all findings collected"));
}

#[tokio::test]
async fn queued_complete_publishes_on_speech_gap() {
    let (_dir, _store, host, soul) = open_work();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    host.set_report_sink(tx);
    drop(host.mark_user_speaking(true));
    let job = public_start(&host, soul, "report");
    let queued = host.complete(job.id, "all findings collected").unwrap();
    assert_eq!(queued.inner_intent.as_deref(), Some("complete_queued"));
    assert!(
        rx.try_recv().is_err(),
        "queued complete must not publish yet"
    );
    drop(host.mark_user_speaking(false));
    let published = rx.recv().await.unwrap();
    assert_eq!(published.soul_id, soul);
    assert!(published.speech.contains("all findings collected"));
}

#[tokio::test]
async fn surface_router_overwrites_foreign_soul_id() {
    let (_dir, _store, host, soul) = open_work();
    let foreign = SoulId::new();
    let registry = Arc::new(ToolRegistry::new());
    let capture = Arc::new(CaptureInvoke {
        last: parking_lot::Mutex::new(None),
    });
    registry.register_with(
        utility_time_def(),
        Arc::clone(&capture) as Arc<dyn ToolInvoke>,
    );
    let router = WorkSurfaceRouter::new(host, registry, soul, 4);
    router
        .on_tool("utility.time", json!({ "soul_id": foreign.to_string() }), 0)
        .await
        .unwrap();
    let args = capture.last.lock().clone().unwrap();
    assert_eq!(args["soul_id"], json!(soul.to_string()));
}

#[test]
fn interrupt_recovery_and_tool_failure_use_different_wording() {
    let (_dir, store, host, soul) = open_work();
    let job = public_start(&host, soul, "cleanup");
    store.set_status(job.id, JobStatus::Running, None).unwrap();
    let fail = host.fail(job.id, "disk full").unwrap();
    assert!(fail.speech.contains("the task failed"));
    assert!(!fail.speech.contains("stopped"));
    let running = public_start(&host, soul, "another");
    store
        .set_status(running.id, JobStatus::Running, None)
        .unwrap();
    let reports = host.recover_interrupted().unwrap();
    assert_eq!(reports.len(), 1);
    assert!(reports[0].speech.contains("stopped"));
    assert!(!reports[0].speech.contains("the task failed"));
}

#[test]
fn user_facing_strings_say_task_not_job() {
    let (_dir, store, host, soul) = open_work();
    let job = public_start(&host, soul, "cleanup");
    let fail = host.fail(job.id, "disk full").unwrap();
    assert!(fail.speech.contains("the task failed"));
    assert!(!fail.speech.contains("the job failed"));
    let running = public_start(&host, soul, "another");
    store
        .set_status(running.id, JobStatus::Running, None)
        .unwrap();
    let reports = host.recover_interrupted().unwrap();
    assert_eq!(reports.len(), 1);
    assert!(reports[0].speech.contains("the task"));
    assert!(!reports[0].speech.contains("the job"));
}

#[tokio::test]
async fn mutating_work_waits_for_plan_approval() {
    let (_dir, _store, host, soul) = open_work();
    let job = public_start(&host, soul, "rewrite docs");
    assert!(!host.mutating_work_allowed(job.id).unwrap());
    assert!(matches!(
        host.require_mutating_allowed(job.id),
        Err(crate::WorkError::PlanNotApproved)
    ));
    let presented = host
        .present_plan(job.id, "1. edit README\n2. run tests")
        .unwrap();
    assert!(presented.speech.contains("here's the plan"));
    assert!(!host.mutating_work_allowed(job.id).unwrap());
    host.answer(job.id, "please plan_approved thanks").unwrap();
    assert!(!host.mutating_work_allowed(job.id).unwrap());
    host.approve_plan(job.id).unwrap();
    host.require_mutating_allowed(job.id).unwrap();

    let registry = Arc::new(ToolRegistry::new());
    register_work_tools(&registry, Arc::clone(&host), PathBuf::from("/tmp/skills"));
    let names: Vec<String> = registry
        .schemas(ene_registry::Layer::Surface)
        .iter()
        .filter_map(|schema| {
            schema
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert!(names.iter().any(|n| n == "delegate.approve_plan"));
}

#[test]
fn mcp_store_round_trips_args() {
    let dir = TempDir::new().unwrap();
    let store = WorkStore::open(dir.path().join("companions.db")).unwrap();
    store
        .replace_mcp(&[crate::McpServer {
            id: "git".into(),
            transport: "stdio".into(),
            command: Some("npx".into()),
            args: vec!["-y".into(), "git-mcp".into()],
            url: None,
            enabled: true,
        }])
        .unwrap();
    let listed = store.list_mcp().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "git");
    assert_eq!(listed[0].args, vec!["-y".to_owned(), "git-mcp".to_owned()]);
    store.replace_mcp(&[]).unwrap();
    assert!(store.list_mcp().unwrap().is_empty());
}

struct BgInvoke {
    phase: parking_lot::Mutex<std::collections::HashMap<String, String>>,
}

#[async_trait]
impl ToolInvoke for BgInvoke {
    async fn invoke(&self, name: &str, _args: Value) -> Result<Value, String> {
        Err(format!("{name} is background-only"))
    }

    async fn start_background(
        &self,
        execution_id: &str,
        _name: &str,
        _args: Value,
        _deadline_ms: Option<u64>,
    ) -> Result<(), String> {
        self.phase
            .lock()
            .insert(execution_id.to_owned(), "running".to_owned());
        Ok(())
    }

    async fn cancel_background(&self, execution_id: &str) -> Result<String, String> {
        let mut phase = self.phase.lock();
        match phase.get(execution_id).map(String::as_str) {
            None => Ok("unknown".to_owned()),
            Some("completed" | "cancelled") => Ok("already_terminal".to_owned()),
            Some(_) => {
                phase.insert(execution_id.to_owned(), "cancelled".to_owned());
                Ok("cancelled".to_owned())
            }
        }
    }

    async fn status_background(
        &self,
        execution_id: &str,
    ) -> Result<(String, Option<String>), String> {
        Ok((
            self.phase
                .lock()
                .get(execution_id)
                .cloned()
                .unwrap_or_else(|| "unknown".to_owned()),
            None,
        ))
    }
}

fn bg_def() -> ToolDefinition {
    ToolDefinition {
        name: "bg.sleep".to_owned(),
        description: "sleep in the background".to_owned(),
        parameters: json!({"type":"object"}),
        output: json!({"type":"object"}),
        side_effects: Vec::new(),
        source: ToolSource::Plugin {
            plugin_id: "tool.bg".to_owned(),
        },
        timeout_ms: Some(2_000),
        sensitivity: Sensitivity::None,
        category: String::new(),
        keywords: Vec::new(),
        examples: Vec::new(),
        background: true,
    }
}

#[tokio::test]
async fn background_tool_releases_the_turn_and_reports_once() {
    let (_dir, _store, host, soul) = open_work();
    host.set_report_sink({
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        tx
    });
    let registry = Arc::new(ToolRegistry::new());
    let invoke = Arc::new(BgInvoke {
        phase: parking_lot::Mutex::new(std::collections::HashMap::new()),
    });
    registry.register_with(bg_def(), Arc::clone(&invoke) as Arc<dyn ToolInvoke>);
    let router = WorkSurfaceRouter::new(Arc::clone(&host), Arc::clone(&registry), soul, 8);
    let outcome = router
        .on_tool("bg.sleep", json!({"ms": 1}), 0)
        .await
        .unwrap();
    let SurfaceToolOutcome::Result(value) = outcome else {
        panic!("expected immediate result, got {outcome:?}");
    };
    assert_eq!(value["status"], "started");
    let execution_id = value["execution_id"].as_str().unwrap().to_owned();
    invoke
        .phase
        .lock()
        .insert(execution_id.clone(), "completed".to_owned());
    tokio::time::sleep(StdDuration::from_millis(80)).await;
    let row = host.tool_execution(&execution_id).unwrap().unwrap();
    assert!(row.status.is_terminal());
    let first = host
        .apply_tool_completion(
            &execution_id,
            crate::ToolExecStatus::Completed,
            None,
            "again",
        )
        .unwrap();
    assert!(first.is_none());
}

#[tokio::test]
async fn background_cancel_unknown_and_timeout_are_distinct() {
    let (_dir, _store, host, soul) = open_work();
    assert_eq!(host.cancel_tool_execution("missing").unwrap(), "unknown");
    host.begin_tool_execution(&crate::NewToolExecution {
        execution_id: "exec-t".to_owned(),
        job_id: None,
        soul_id: soul,
        tool_name: "bg.sleep".to_owned(),
        plugin_id: Some("tool.bg".to_owned()),
        call_id: "c".to_owned(),
    })
    .unwrap();
    let report = host.timeout_tool_execution("exec-t").unwrap().unwrap();
    assert!(report.speech.contains("timed out"));
    assert_eq!(
        host.tool_execution("exec-t").unwrap().unwrap().status,
        crate::ToolExecStatus::TimedOut
    );
    host.begin_tool_execution(&crate::NewToolExecution {
        execution_id: "exec-c".to_owned(),
        job_id: None,
        soul_id: soul,
        tool_name: "bg.sleep".to_owned(),
        plugin_id: Some("tool.bg".to_owned()),
        call_id: "c".to_owned(),
    })
    .unwrap();
    let crash = host
        .crash_tool_execution("exec-c", "plugin_crash")
        .unwrap()
        .unwrap();
    assert!(crash.speech.contains("stopped"));
    let reports = host.recover_tool_executions().unwrap();
    assert!(reports.is_empty());
}
