use thiserror::Error;

/// Body / voice pipeline failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BodyError {
    #[error("unknown body {0}")]
    UnknownBody(String),
    #[error("unknown expression {0}")]
    UnknownExpression(String),
    #[error("unknown motion {0}")]
    UnknownMotion(String),
    #[error("voice disabled")]
    VoiceDisabled,
    #[error("another body is speaking")]
    SpeakerBusy,
    #[error("tts unavailable")]
    TtsUnavailable,
    #[error("asr unavailable")]
    AsrUnavailable,
}
