//! CLI client can reach the same core API surface as desktop/Web.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "integration tests fail fast"
)]
#![deny(unsafe_code)]

use ene_api::{ApiClient, CreateSessionRequest, MessageMode, MessageRequest};
use ene_ctl::core::{read_api_ready, wait_for_api_json};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;
use tempfile::TempDir;

struct KillOnDrop(Option<Child>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            if child.kill().is_err() {
                // Already exited.
            }
            drop(child.wait());
        }
    }
}

fn ene_core_bin() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_ene_core") {
        return PathBuf::from(path);
    }
    let mut path = std::env::current_exe().expect("current_exe");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("ene-core");
    path
}

#[tokio::test]
async fn ctl_client_lists_tools_and_debug_spans() {
    let dir = TempDir::new().unwrap();
    let child = Command::new(ene_core_bin())
        .arg("--data-dir")
        .arg(dir.path())
        .env("RUST_LOG", "error")
        .spawn()
        .expect("spawn ene-core");
    let _child = KillOnDrop(Some(child));
    let ready = wait_for_api_json(&dir.path().join("api.json"), Duration::from_secs(20))
        .await
        .unwrap();
    let api = read_api_ready(&dir.path().join("api.json")).unwrap();
    assert_eq!(api.url, ready.url);
    let client = ApiClient::new(ready.url, ready.token.unwrap_or_default(), "cli");
    client.health().await.unwrap();
    let tools = client.list_tools().await.unwrap();
    assert!(!tools.items.is_empty());
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id: ene_session::SoulId::new().to_string(),
            title: None,
        })
        .await
        .unwrap();
    client
        .send_message(
            &session.id,
            &MessageRequest {
                text: "ctl hello".into(),
                mode: MessageMode::Prompt,
                input_modality: None,
            },
            None,
        )
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let history = client.history(&session.id, "surface").await.unwrap();
        if history
            .messages
            .iter()
            .any(|message| message.role == "assistant")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let spans = client.diag_spans().await.unwrap();
    assert!(!spans.items.is_empty());
}
