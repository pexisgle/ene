//! Delegation, jobs, schedules, skills, MCP bindings, and vision dual-path.

#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "unit tests use unwrap/expect/panic for assertions"
    )
)]
#![deny(unsafe_code)]

mod computer;
mod error;
mod host;
mod mcp;
mod observe;
mod questions;
mod router;
mod runner;
mod schedule;
mod skill;
mod speech_gate;
mod spill;
mod store;
mod task;
mod tools;
mod types;
mod vision;
mod workflow;

pub use computer::{
    ActionKind, ComputerAction, ComputerError, ElementRef, GrantId, GrantScope, ObservationId,
    TaskGrant, WindowIdentity, verify_focus, verify_grant, verify_stale,
};
pub use error::WorkError;
pub use host::{
    DelegationHost, StartDelegation, SurfaceCallKind, UpgradeRequest, def_is_side_effect,
    fold_brief, layer_for_call, question_timed_out, should_upgrade_steps, soul_artifacts_dir,
    surface_call_kind, workspace_root,
};
pub use mcp::{
    McpCatalogAuth, McpCatalogEntry, McpProfile, McpServer, McpTool, ScriptedMcp, mcp_catalog,
    register_mcp_tools,
};
pub use observe::{
    ObservationPipeline, ObserveAction, ObserveError, ObserveSkip, contains_raw_screenshot,
    observation_send_label, title_reaches_model, vision_payload,
};
pub use questions::{combine_questions, route_combined_answers};
pub use router::WorkSurfaceRouter;
pub use runner::{JobDrive, drive_job};
pub use schedule::{
    FiredSchedule, QuietWindow, catch_up_missed, catch_up_missed_with_quiet, fire_due,
    reminder_report,
};
pub use skill::{
    InstalledSkill, SkillMeta, catalog, install_skill_dir, load_skill, match_skills,
    parse_skill_md, read_skill_file, skill_active_blocks, skill_catalog_blocks,
    skill_context_lines, skill_emotion_notes, skill_matches, skill_proactive_hints,
};
pub use speech_gate::SpeechGate;
pub use spill::{
    DEFAULT_HARD_LIMIT_BYTES, DEFAULT_SOFT_LIMIT_BYTES, SpillResult, bound_brief, spill_tool_output,
};
pub use store::{MailboxEntry, WorkStore, next_fire};
pub use task::{
    ArtifactRef, Task, TaskContract, TaskError, TaskState, transition, verify_artifact,
};
pub use tools::{register_work_tools, surface_shows_delegate};
pub use types::{
    Artifact, ArtifactKind, CombinedQuestionTurn, CompanionReport, DelegationMode, Job, JobStatus,
    NewJob, NewSchedule, NewToolExecution, OpenQuestion, Schedule, ScheduleAction, ToolExecStatus,
    ToolExecution, UpgradeReason, WorkDelegationSettings,
};
pub use vision::{
    MINIMAL_PNG, ScreenshotError, capture_screenshot, observe_screen, observe_screen_with_activity,
    register_screenshot_tool, screenshot_is_job_or_surface, screenshot_png,
};
pub use workflow::{BookmarkFill, BookmarkSection, deliver_bookmark_workflow, fill_bookmark_job};

pub use ene_session::{DelegationId, SoulId};

#[cfg(test)]
mod tests;
