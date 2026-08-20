use crate::types::Problem;
use thiserror::Error;

/// Client-side API failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ApiError {
    #[error("http {status}: {error_class}: {title}")]
    Problem {
        status: u16,
        error_class: String,
        title: String,
        body: Box<Problem>,
    },
    #[error("transport: {0}")]
    Transport(String),
    #[error("codec: {0}")]
    Codec(String),
    #[error("websocket: {0}")]
    Websocket(String),
}

impl ApiError {
    #[must_use]
    pub fn error_class(&self) -> &str {
        match self {
            Self::Problem { error_class, .. } => error_class,
            Self::Transport(_) => "transport",
            Self::Codec(_) => "codec",
            Self::Websocket(_) => "websocket",
        }
    }

    pub fn from_problem(status: u16, body: Problem) -> Self {
        Self::Problem {
            status,
            error_class: body.error_class.clone(),
            title: body.title.clone(),
            body: Box::new(body),
        }
    }
}
