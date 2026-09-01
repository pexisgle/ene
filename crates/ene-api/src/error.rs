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

#[cfg(test)]
mod tests {
    use super::ApiError;
    use crate::types::Problem;

    #[test]
    fn error_class_matches_variant() {
        assert_eq!(ApiError::Transport("x".into()).error_class(), "transport");
        assert_eq!(ApiError::Codec("x".into()).error_class(), "codec");
        assert_eq!(ApiError::Websocket("x".into()).error_class(), "websocket");
        let problem = ApiError::from_problem(401, Problem::new(401, "auth", "nope"));
        assert_eq!(problem.error_class(), "auth");
    }
}
