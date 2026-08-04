//! Persistent scheduler domain model — re-exported from `ene-core`.
//!
//! Pure domain vocabulary for persistent schedules. This module stays so
//! existing `ene_store::schedule::*` / `ene_store::Schedule`-style import
//! paths keep working unchanged.

pub use ene_core::{
    NewSchedule, Schedule, ScheduleAction, ScheduleConfirmation, ScheduleError, ScheduleKind,
    ScheduleRun, ScheduleRunStatus,
};
