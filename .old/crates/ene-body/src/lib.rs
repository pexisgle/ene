//! Performance protocol, emotion→expression mapping, duplex voice (W4).
//! Rendering stays in `ene-vrm`; this crate owns the queue language and
//! the core-side voice state machine.

#![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests may fail fast"))]
#![deny(unsafe_code)]

mod bus;
mod config;
mod error;
mod lipsync;
mod map;
mod queue;
mod stage;
mod voice;

pub use bus::{IssuedCommand, PerformanceBus, SharedBus};
pub use config::{
    AutonomySettings, BargeInSettings, BodySettings, FallbackSettings, RenderSettings,
    VoiceInputSettings, VoiceSettings,
};
pub use error::BodyError;
pub use lipsync::{LipSyncAnalyzer, VisemeWeights};
pub use map::{BodyCatalog, BodyKind, MappedExpression};
pub use queue::{
    EmotionCue, LookTarget, MotionLayer, PerformanceCommand, Posture, Viseme, Vitality,
};
pub use stage::Stage;
pub use voice::{
    AsrEngine, DuplexState, EnergyVad, InputEffect, ScriptedAsr, ScriptedTts, SpeakOutput,
    TtsEngine, VoiceRuntime,
};

#[cfg(test)]
mod tests;
