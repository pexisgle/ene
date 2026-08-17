use ene_api::{ApiClient, ApiError, Page, Problem, SessionView};
use reqwest::Method;
use serde::de::DeserializeOwned;

pub async fn search_sessions(
    client: &ApiClient,
    query: &str,
) -> Result<Page<SessionView>, ApiError> {
    match search_sessions_api(client, query).await {
        Ok(page) => Ok(page),
        Err(ApiError::Problem { status: 404, .. }) => filter_sessions(client, query).await,
        Err(err) => Err(err),
    }
}

pub async fn split_session(
    client: &ApiClient,
    id: &str,
) -> Result<ene_api::SplitSessionResponse, ApiError> {
    client.split_session(id).await
}

async fn search_sessions_api(
    client: &ApiClient,
    query: &str,
) -> Result<Page<SessionView>, ApiError> {
    send_json(client.request(
        Method::GET,
        &format!("/api/v1/sessions?q={}", url_encode(query)),
    ))
    .await
}

async fn filter_sessions(client: &ApiClient, query: &str) -> Result<Page<SessionView>, ApiError> {
    let page = client.list_sessions(None).await?;
    let items = page
        .items
        .into_iter()
        .filter(|session| session_matches(query, session))
        .collect();
    Ok(Page::of(items))
}

#[must_use]
pub fn session_matches(needle: &str, session: &SessionView) -> bool {
    let needle = needle.to_lowercase();
    session
        .title
        .as_ref()
        .is_some_and(|title| title.to_lowercase().contains(&needle))
        || session.id.to_lowercase().contains(&needle)
}

async fn send_json<T: DeserializeOwned>(builder: reqwest::RequestBuilder) -> Result<T, ApiError> {
    let response = builder
        .send()
        .await
        .map_err(|err| ApiError::Transport(err.to_string()))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|err| ApiError::Transport(err.to_string()))?;
    if status.is_success() {
        serde_json::from_slice(&bytes).map_err(|err| ApiError::Codec(err.to_string()))
    } else {
        let problem = serde_json::from_slice::<Problem>(&bytes).unwrap_or_else(|_| {
            Problem::new(
                status.as_u16(),
                "fault",
                std::str::from_utf8(&bytes).unwrap_or("request failed"),
            )
        });
        Err(ApiError::from_problem(status.as_u16(), problem))
    }
}

fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push_str(&percent_encode_byte(byte));
            }
        }
    }
    out
}

fn percent_encode_byte(byte: u8) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(3);
    out.push('%');
    out.push(HEX[(byte >> 4) as usize] as char);
    out.push(HEX[(byte & 0x0f) as usize] as char);
    out
}

trait ApiRequestExt {
    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder;
}

impl ApiRequestExt for ApiClient {
    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .request(method, format!("{}{path}", self.base()))
            .bearer_auth(self.token())
            .header("X-Client-Id", self.client_id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_session(id: &str, title: Option<&str>) -> SessionView {
        SessionView {
            id: id.to_owned(),
            soul_id: "soul".to_owned(),
            kind: "chat".to_owned(),
            title: title.map(str::to_owned),
            created_at: "now".to_owned(),
            archived: false,
            next_seq: 0,
            ended_at: None,
            end_reason: None,
        }
    }

    #[test]
    fn session_matches_title_case_insensitive() {
        let session = sample_session("s1", Some("Hello World"));
        assert!(session_matches("hello", &session));
        assert!(session_matches("WORLD", &session));
        assert!(!session_matches("missing", &session));
    }

    #[test]
    fn session_matches_id() {
        let session = sample_session("abc-123", None);
        assert!(session_matches("abc", &session));
        assert!(!session_matches("xyz", &session));
    }

    #[test]
    fn url_encode_spaces_and_unicode() {
        assert_eq!(url_encode("hello world"), "hello%20world");
        assert_eq!(url_encode("a&b=c"), "a%26b%3Dc");
    }
}
