//! Companion layer: soul, hybrid affect, memory (with `scope`), inner channel,
//! proactive speech, and character packages.

#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "tests may fail fast"
    )
)]
#![deny(unsafe_code)]

mod affect;
mod classify;
mod config;
mod error;
mod ids;
mod inner;
mod memory;
mod package;
mod proactive;
mod runtime;
mod soul;
mod store;
mod tools;

pub use affect::{
    AffectBaseline, AffectPresentation, AffectProposal, AffectState, ExpressionArbiter, VOCABULARY,
    apply_self_report, apply_turn_signals, project_decay,
};
pub use classify::{ClassifyModel, ClassifyTask, ScriptedClassify};
pub use config::{
    CharacterSettings, ForgettingMode, MindSettings, ProactiveSettings, RecallSettings,
    WorldStateSettings,
};
pub use error::CompanionError;
pub use ids::{CandidateId, MemoryId};
pub use inner::{
    EmotionReport, derive_thought_from_thinking, model_visible_for, parse_emotion_report,
    split_surface_and_inner,
};
pub use memory::{
    ArbitrateOutcome, JournalAction, MemoryCandidate, MemoryKind, MemoryRecord, MemoryScope,
    MemorySource, NewMemory, RecalledMemory, apply_forget_request, arbitrate,
    deterministic_extract, extract_turn,
};
pub use package::{
    InstalledPackage, PackageKind, avatar_path_for_install, compose_soul_and_body, content_digest,
    display_name_for_install, export_dir, import_v3, install_archive, localized_display_name,
    looks_like_package_zip, looks_like_zip, pack_archive, soul_from_install,
};
pub use proactive::{
    ActivitySnapshot, GateRejectReason, ProactiveConfirmation, ProactiveContext, ProactiveDecision,
    ProactiveDecisionOutcome, ProactiveObservation, ProactiveSkipReason, ProactiveSuppressionState,
    SILENT_TOKEN, ScreenSummaryStatus, WorldStateMemory, WorldStateSnapshot,
    build_proactive_context, classify_confirmation_prefix, decide_proactive_speech,
    evaluate_deterministic_gates, evaluate_quiet_hours,
};
pub use runtime::CompanionRuntime;
pub use soul::{NewSoul, Soul};
pub use store::CompanionStore;
pub use tools::{QueryEmbed, SlotQueryEmbed, register_memory_tools, surface_hides_write_shared};

pub use ene_session::SoulId;

#[cfg(test)]
mod tests;
