use thiserror::Error;

/// Error type for the *cognition* domain: recall, memory writing/arbitration,
/// forgetting, character-card memory sync, the affect/emotion classifier,
/// context budgeting/compression, commitments, and the turn engine.
///
/// This is the error type nearly every function in those submodules returns
/// (directly, or via the [`CognitionError`] alias) — it is the common
/// currency *within* the cognition half of `ene-mind`, not the crate's
/// outward-facing boundary type. It intentionally does not know about
/// session lifecycle (split/compression bookkeeping, history persistence):
/// that is [`crate::session::EneSessionError`]'s job.
///
/// External callers (`ene-runtime` and other embedders) should generally
/// match on [`MindError`] instead, which composes this type with
/// `EneSessionError` behind one `#[non_exhaustive]`-friendly surface. Code
/// living inside `ene-mind`'s cognition submodules should keep using this
/// type (or the `CognitionError` alias) directly — wrapping every internal
/// call site in `MindError` this early would lose the ability to match on
/// cognition-specific variants without re-destructuring.
#[derive(Error, Debug)]
pub enum EneCognitionError {
    /// Memory operation failed via the `MemoryPort` abstraction.
    ///
    /// Used by all cognitive-logic modules that call `&dyn MemoryPort`
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

/// Shorter alias for [`EneCognitionError`], used pervasively within the
/// cognition submodules' own signatures (`recall`, `memory_writer`,
/// `context`, `character`, `emotion`, `commitments`, `engine`). Same type,
/// just less noisy at internal call sites — see [`EneCognitionError`]'s
/// docs for why this is *not* the crate boundary type.
pub type CognitionError = EneCognitionError;

/// Single public error type for the `ene-mind` crate boundary (API v1).
///
/// This — not [`EneCognitionError`] — is the type external embedders
/// (`ene-runtime` in particular, via `#[from] ene_mind::MindError` on
/// `EneRuntimeError`) should match against. It projects both halves of
/// `ene-mind`'s internal error surface into one boundary type:
///
/// - [`EneCognitionError`]: recall, memory writing/arbitration, forgetting,
///   character memory sync, affect/emotion, context budgeting/compression,
///   commitments, and the turn engine.
/// - [`crate::session::EneSessionError`]: session lifecycle — history
///   bookkeeping, manual split/compression triggers.
///
/// Kept as a thin two-variant wrapper rather than merged into
/// `EneCognitionError` because the two halves have non-overlapping
/// concerns and separate owners internally; merging them would force every
/// cognition-submodule call site (the vast majority of `ene-mind`'s
/// internal `Result` usage) to also account for session-lifecycle variants
/// that can never occur there.
#[derive(Error, Debug)]
pub enum MindError {
    /// Cognitive pipeline failure — see [`EneCognitionError`].
    #[error(transparent)]
    Cognition(#[from] EneCognitionError),
    /// Session / split / compression failure — see
    /// [`crate::session::EneSessionError`].
    #[error(transparent)]
    Session(#[from] crate::session::EneSessionError),
}
