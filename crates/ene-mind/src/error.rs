use thiserror::Error;

/// Error types for the cognitive runtime.
#[derive(Error, Debug)]
pub enum EneCognitionError {
    /// Memory operation failed.
    #[error(transparent)]
    Memory(#[from] ene_store::EneMemoryError),

    /// Memory operation failed via the `MemoryPort` abstraction (#270).
    ///
    /// Used by cognitive-logic modules (recall, arbiter, forgetting,
    /// character sync, journal, reflection) that call `&dyn MemoryPort`
    /// instead of the concrete `ene_store::MemoryStore`.
    #[error(transparent)]
    MemoryPort(#[from] ene_core::MemoryPortError),

    /// Configuration error.
    #[error(transparent)]
    Config(#[from] ene_config::EneConfigError),

    /// Provider error.
    #[error(transparent)]
    Provider(#[from] ene_ai::LlmProviderError),

    /// Embedding provider error.
    #[error(transparent)]
    Embedding(#[from] ene_ai::EmbeddingError),

    /// Memory extraction failed.
    #[error("Memory extraction failed: {0}")]
    ExtractionFailed(String),

    /// Memory arbitration failed.
    #[error("Memory arbitration failed: {0}")]
    ArbitrationFailed(String),

    /// Recall planning failed.
    #[error("Recall planning failed: {0}")]
    RecallFailed(String),

    /// Emotion computation failed.
    #[error("Emotion computation failed: {0}")]
    EmotionFailed(String),

    /// Affect classifier LLM call failed.
    #[error(transparent)]
    Classifier(#[from] crate::emotion::classifier::ClassifierError),

    /// Prompt composition failed.
    #[error("Prompt composition failed: {0}")]
    PromptBuildError(String),

    /// Context budget exceeded.
    #[error("Context budget exceeded: {0}")]
    BudgetExceeded(String),

    /// Invalid state transition.
    #[error("Invalid state transition: {0}")]
    InvalidState(String),

    /// Required provider or resource is missing.
    #[error("Missing required provider: {0}")]
    MissingProvider(String),

    /// Catch-all for other errors.
    #[error("Other error: {0}")]
    Other(String),

    /// Aggregated error from multiple operations (e.g. batch cancellation).
    #[error("Aggregated error: {0}")]
    Aggregate(String),
}

/// Type alias for internal module usage.
///
/// [`EneCognitionError`] is the canonical public name for the crate boundary;
/// `CognitionError` is a shorter alias used pervasively within the crate internals.
pub type CognitionError = EneCognitionError;

/// Single public error type for the `ene-mind` crate boundary (API v1 / #118).
#[derive(Error, Debug)]
pub enum MindError {
    /// Cognitive pipeline failure.
    #[error(transparent)]
    Cognition(#[from] EneCognitionError),
    /// Session / split / compression failure.
    #[error(transparent)]
    Session(#[from] crate::session::EneSessionError),
}
