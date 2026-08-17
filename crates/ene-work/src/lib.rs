//! Delegation, jobs, schedules, skills, MCP bindings, and vision dual-path.

#![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests may fail fast"))]
#![deny(unsafe_code)]

mod error;
mod host;
mod mcp;
mod router;
mod schedule;
mod skill;
mod store;
mod tools;
mod types;
mod vision;

pub use error::WorkError;
pub use host::{
    DelegationHost, StartDelegation, SurfaceCallKind, UpgradeRequest, def_is_side_effect,
    fold_brief, layer_for_call, question_timed_out, should_upgrade_steps, surface_call_kind,
};
pub use mcp::{McpProfile, McpTool, ScriptedMcp, register_mcp_tools};
pub use router::WorkSurfaceRouter;
pub use schedule::{FiredSchedule, QuietWindow, catch_up_missed, fire_due, reminder_report};
pub use skill::{
    InstalledSkill, SkillMeta, catalog, install_skill_dir, load_skill, parse_skill_md,
};
pub use store::{WorkStore, next_fire};
pub use tools::{register_work_tools, surface_shows_delegate};
pub use types::{
    Artifact, ArtifactKind, CompanionReport, DelegationMode, Job, JobStatus, NewJob, NewSchedule,
    Schedule, ScheduleAction, UpgradeReason,
};
pub use vision::{
    PlaceholderScreenshot, observe_screen, register_screenshot_tool, screenshot_is_job_or_surface,
};

pub use ene_session::{DelegationId, SoulId};

#[cfg(test)]
mod tests;
