use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use ene_api::Problem;

/// Reject an HTTP call with a problem+json body.
#[derive(Debug, Clone)]
pub struct ApiReject(pub Box<Problem>);

impl ApiReject {
    #[must_use]
    pub fn new(status: StatusCode, error_class: &str, title: &str) -> Self {
        Self(Box::new(Problem::new(status.as_u16(), error_class, title)))
    }

    #[must_use]
    pub fn with_turn(mut self, turn_id: impl Into<String>) -> Self {
        self.0.turn_id = Some(turn_id.into());
        self
    }

    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.0.detail = Some(detail.into());
        self
    }
}

impl IntoResponse for ApiReject {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut response = Json(self.0).into_response();
        *response.status_mut() = status;
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}

pub fn map_kernel(err: &ene_kernel::KernelError) -> ApiReject {
    use ene_kernel::KernelError;
    match err {
        KernelError::LaneBusy { turn_id } => {
            ApiReject::new(StatusCode::CONFLICT, err.error_class(), "lane busy")
                .with_turn(turn_id.to_string())
        }
        KernelError::NoActiveOperation { .. } => ApiReject::new(
            StatusCode::CONFLICT,
            err.error_class(),
            "no active operation",
        ),
        KernelError::InvalidMessage(msg) => ApiReject::new(
            StatusCode::BAD_REQUEST,
            err.error_class(),
            "invalid message",
        )
        .with_detail(msg.clone()),
        KernelError::Closed | KernelError::ShuttingDown => {
            ApiReject::new(StatusCode::CONFLICT, err.error_class(), "lane closed")
        }
        KernelError::NothingToCompact => ApiReject::new(
            StatusCode::CONFLICT,
            err.error_class(),
            "nothing to compact",
        ),
        KernelError::Session(ene_session::SessionError::SessionNotFound(id)) => {
            ApiReject::new(StatusCode::NOT_FOUND, "not_found", "session not found")
                .with_detail(id.clone())
        }
        other => ApiReject::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            other.error_class(),
            "fault",
        )
        .with_detail(other.to_string()),
    }
}

pub fn not_found(what: &str) -> ApiReject {
    ApiReject::new(StatusCode::NOT_FOUND, "not_found", what)
}

pub fn bad_request(class: &str, title: &str) -> ApiReject {
    ApiReject::new(StatusCode::BAD_REQUEST, class, title)
}

pub fn conflict(class: &str, title: &str) -> ApiReject {
    ApiReject::new(StatusCode::CONFLICT, class, title)
}

pub fn unauthorized() -> ApiReject {
    ApiReject::new(StatusCode::UNAUTHORIZED, "unauthorized", "invalid token")
}
