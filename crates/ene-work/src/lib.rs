//! Delegation, jobs, schedules, skills, MCP bindings, and vision dual-path.

#![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests may fail fast"))]
#![deny(unsafe_code)]

mod error;
mod host;
mod mcp;
mod questions;
mod router;
mod schedule;
mod skill;
mod speech_gate;
mod spill;
mod store;
mod tools;
mod types;
mod vision;
mod workflow;

pub use error::WorkError;
pub use host::{
    DelegationHost, StartDelegation, SurfaceCallKind, UpgradeRequest, def_is_side_effect,
    fold_brief, layer_for_call, question_timed_out, should_upgrade_steps, surface_call_kind,
};
pub use mcp::{McpProfile, McpTool, ScriptedMcp, register_mcp_tools};
pub use questions::{combine_questions, route_combined_answers};
pub use router::WorkSurfaceRouter;
pub use schedule::{FiredSchedule, QuietWindow, catch_up_missed, fire_due, reminder_report};
pub use skill::{
    InstalledSkill, SkillMeta, catalog, install_skill_dir, load_skill, parse_skill_md,
};
pub use speech_gate::SpeechGate;
pub use spill::{
    bound_brief, spill_tool_output, DEFAULT_HARD_LIMIT_BYTES, DEFAULT_SOFT_LIMIT_BYTES, SpillResult,
};
pub use store::{MailboxEntry, WorkStore, next_fire};
pub use tools::{register_work_tools, surface_shows_delegate};
pub use types::{
    Artifact, ArtifactKind, CombinedQuestionTurn, CompanionReport, DelegationMode, Job,
    JobStatus, NewJob, NewSchedule, OpenQuestion, Schedule, ScheduleAction, UpgradeReason,
    WorkDelegationSettings,
};
pub use vision::{
    PlaceholderScreenshot, observe_screen, register_screenshot_tool, screenshot_is_job_or_surface,
};
pub use workflow::{BookmarkSection, deliver_bookmark_workflow};

pub use ene_session::{DelegationId, SoulId};

#[cfg(test)]
mod tests;
