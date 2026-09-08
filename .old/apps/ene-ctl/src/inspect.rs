use ene_api::{ApiClient, ApiError, HistoryResponse, SessionView};
use serde::Serialize;

/// Detail-depth history of a job's child session.
#[derive(Debug, Serialize)]
pub struct DelegationDebug {
    pub session: SessionView,
    pub history: HistoryResponse,
}

/// Load a delegation session by job id or session id, at detail depth.
///
/// # Errors
///
/// Returns a codec error when `id` is a conversation session or nothing matches,
/// or API failures from `ene-core`.
pub async fn show_delegation(client: &ApiClient, id: &str) -> Result<DelegationDebug, ApiError> {
    let session = resolve_delegation_session(client, id).await?;
    let history = client.history(&session.id, "detail").await?;
    Ok(DelegationDebug { session, history })
}

async fn resolve_delegation_session(client: &ApiClient, id: &str) -> Result<SessionView, ApiError> {
    match client.get_session(id).await {
        Ok(session) => require_delegation(session),
        Err(ApiError::Problem { status: 404, .. }) => {
            let page = client.list_sessions(None).await?;
            find_delegation_session(id, &page.items)
                .cloned()
                .ok_or_else(|| ApiError::Codec(format!("no delegation session for {id}")))
        }
        Err(err) => Err(err),
    }
}

fn require_delegation(session: SessionView) -> Result<SessionView, ApiError> {
    if session.kind == "delegation" {
        Ok(session)
    } else {
        Err(ApiError::Codec(format!(
            "session {} is {} (need a delegation session or job id)",
            session.id, session.kind
        )))
    }
}

#[must_use]
fn find_delegation_session<'a>(id: &str, sessions: &'a [SessionView]) -> Option<&'a SessionView> {
    sessions.iter().find(|session| {
        session.kind == "delegation"
            && (session.id == id || session.delegation_id.as_deref() == Some(id))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, kind: &str, delegation_id: Option<&str>) -> SessionView {
        SessionView {
            id: id.to_owned(),
            soul_id: "soul".to_owned(),
            kind: kind.to_owned(),
            title: None,
            created_at: "now".to_owned(),
            archived: false,
            next_seq: 0,
            ended_at: None,
            end_reason: None,
            delegation_id: delegation_id.map(str::to_owned),
        }
    }

    #[test]
    fn finds_child_by_session_or_job_id() {
        let rows = [
            session("conv", "conversation", None),
            session("child", "delegation", Some("job-1")),
        ];
        assert_eq!(
            find_delegation_session("child", &rows).map(|row| row.id.as_str()),
            Some("child")
        );
        assert_eq!(
            find_delegation_session("job-1", &rows).map(|row| row.id.as_str()),
            Some("child")
        );
        assert!(find_delegation_session("conv", &rows).is_none());
        assert!(find_delegation_session("missing", &rows).is_none());
    }

    #[test]
    fn require_delegation_rejects_conversation() {
        let err = require_delegation(session("s1", "conversation", None)).unwrap_err();
        assert!(err.to_string().contains("conversation"));
    }
}
