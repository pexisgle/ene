//! Token endpoint client: authorization-code exchange and refresh.

use serde_json::Value;

use super::FlowError;

/// A token endpoint response carrying a (possibly rotated) token set.
///
/// `refresh_token` is `None` both when the server rotated it away and when
/// it never issued one; the caller decides whether to keep the previous one.
pub(crate) struct OAuthTokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Access-token lifetime in seconds, when the server reports it.
    pub expires_in: Option<i64>,
}

/// Exchanges an authorization `code` for tokens (RFC 6749 §4.1.3).
pub(crate) async fn exchange_code(
    client: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthTokenResponse, FlowError> {
    let response = client
        .post(token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .map_err(|e| FlowError::TokenEndpoint(format!("request failed: {e}")))?;
    parse_token_response(response).await
}

/// Refreshes an access token (RFC 6749 §6).
pub(crate) async fn refresh_token(
    client: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<OAuthTokenResponse, FlowError> {
    let response = client
        .post(token_url)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ])
        .send()
        .await
        .map_err(|e| FlowError::TokenEndpoint(format!("request failed: {e}")))?;
    parse_token_response(response).await
}

/// Parses a token endpoint response, surfacing only non-secret error detail.
///
/// The `error` / `error_description` fields of a rejected exchange are safe
/// to surface; the raw body is never echoed because a misbehaving server
/// could place a token there.
async fn parse_token_response(
    response: reqwest::Response,
) -> Result<OAuthTokenResponse, FlowError> {
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|e| FlowError::MalformedTokenResponse(format!("unreadable response: {e}")))?;
    if !status.is_success() {
        let detail = body
            .get("error_description")
            .and_then(Value::as_str)
            .or_else(|| body.get("error").and_then(Value::as_str))
            .unwrap_or("the token endpoint rejected the request");
        return Err(FlowError::TokenEndpoint(detail.to_string()));
    }
    let access_token = body
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| FlowError::MalformedTokenResponse("missing access_token".to_string()))?;
    let refresh_token = body
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::to_owned);
    // Some servers return `expires_in` as a JSON number, others as a string.
    let expires_in = body.get("expires_in").and_then(Value::as_i64).or_else(|| {
        body.get("expires_in")
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok())
    });
    Ok(OAuthTokenResponse {
        access_token: access_token.to_owned(),
        refresh_token,
        expires_in,
    })
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "tests use unwrap/panic for concise failure messages"
)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Minimal HTTP/1.1 server serving one JSON response body, used to
    /// exercise `exchange_code` / `refresh_token` without external network.
    async fn serve_once(body: &'static str) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0_u8; 1024];
            loop {
                match socket.read(&mut chunk).await.unwrap() {
                    0 => break,
                    n => buf.extend_from_slice(&chunk[..n]),
                }
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(body.as_bytes()).await.unwrap();
            socket.flush().await.unwrap();
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn exchange_code_parses_tokens_and_expiry() {
        let (url, server) =
            serve_once(r#"{"access_token":"at-1","refresh_token":"rt-1","expires_in":3600}"#).await;
        let client = reqwest::Client::new();
        let resp = exchange_code(
            &client,
            &url,
            "client-id",
            "code",
            "verifier",
            "http://127.0.0.1:1/",
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(resp.access_token, "at-1");
        assert_eq!(resp.refresh_token.as_deref(), Some("rt-1"));
        assert_eq!(resp.expires_in, Some(3600));
    }

    #[tokio::test]
    async fn exchange_code_handles_string_expiry_and_absent_refresh() {
        let (url, server) = serve_once(r#"{"access_token":"at-1","expires_in":"1800"}"#).await;
        let client = reqwest::Client::new();
        let resp = exchange_code(
            &client,
            &url,
            "client-id",
            "code",
            "verifier",
            "http://127.0.0.1:1/",
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(resp.access_token, "at-1");
        assert_eq!(resp.refresh_token, None);
        assert_eq!(resp.expires_in, Some(1800));
    }

    #[tokio::test]
    async fn rejected_exchange_surfaces_error_not_body() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0_u8; 1024];
            loop {
                match socket.read(&mut chunk).await.unwrap() {
                    0 => break,
                    n => buf.extend_from_slice(&chunk[..n]),
                }
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let body = r#"{"error":"invalid_grant","error_description":"code expired"}"#;
            let head = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(body.as_bytes()).await.unwrap();
            socket.flush().await.unwrap();
        });
        let client = reqwest::Client::new();
        let err = exchange_code(
            &client,
            &format!("http://{addr}"),
            "c",
            "code",
            "v",
            "http://127.0.0.1:1/",
        )
        .await
        .unwrap_err();
        server.await.unwrap();
        assert!(err.to_string().contains("code expired"));
        assert!(!err.to_string().contains("invalid_grant"));
    }
}
