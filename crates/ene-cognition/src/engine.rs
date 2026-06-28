use crate::{
    character::CharacterProcessor, commitments::CommitmentLedger, context::ContextManager,
    emotion::EmotionEngine, memory_writer::MemoryWriter, output::OutputArbiter,
    pre_turn::PreTurnAnalyzer, prompt_packet::PromptPacket, recall::RecallPlanner,
};

/// Central cognitive engine facade.
///
/// Owns and coordinates all cognitive subsystems. Created once and shared
/// across the streaming lifecycle. Components are populated incrementally
/// across Phases 1–8 of the Cognitive Runtime implementation.
pub struct CognitionEngine {
    /// Pre-turn input analysis.
    pub pre_turn: PreTurnAnalyzer,
    /// Context budget and compression management.
    pub context: ContextManager,
    /// Memory extraction and arbitration.
    pub memory_writer: MemoryWriter,
    /// Memory recall planning.
    pub recall: RecallPlanner,
    /// Deterministic and LLM emotion computation.
    pub emotion: EmotionEngine,
    /// Character identity and lorebook processing.
    pub character: CharacterProcessor,
    /// Sectioned prompt composition.
    pub prompt_packet: PromptPacket,
    /// Expression arbitration and output validation.
    pub output: OutputArbiter,
    /// Companion promise and task tracking.
    pub commitments: CommitmentLedger,
}

impl Default for CognitionEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl CognitionEngine {
    /// Create a new, empty cognitive engine.
    ///
    /// All components start as empty placeholders and will be populated
    /// with configuration and state in later phases.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pre_turn: PreTurnAnalyzer,
            context: ContextManager,
            memory_writer: MemoryWriter,
            recall: RecallPlanner,
            emotion: EmotionEngine,
            character: CharacterProcessor,
            prompt_packet: PromptPacket,
            output: OutputArbiter,
            commitments: CommitmentLedger,
        }
    }
}
