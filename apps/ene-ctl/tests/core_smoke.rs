//! CLI client can reach the same core API surface as desktop/Web.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "integration tests fail fast"
)]
#![deny(unsafe_code)]

use ene_api::{ApiClient, CreateSessionRequest, EndSessionRequest, MessageMode, MessageRequest};
use ene_ctl::core::{read_api_ready, wait_for_api_json};
use ene_daemon as _;
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

fn sibling_bin(name: &str) -> PathBuf {
    if name == "ene-core"
        && let Some(path) = option_env!("CARGO_BIN_EXE_ene_core")
    {
        return PathBuf::from(path);
    }
    if name == "ene-ctl"
        && let Some(path) = option_env!("CARGO_BIN_EXE_ene_ctl")
    {
        return PathBuf::from(path);
    }
    let mut path = std::env::current_exe().expect("current_exe");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(name);
    path
}

fn ene_core_bin() -> PathBuf {
    sibling_bin("ene-core")
}

fn ene_ctl_bin() -> PathBuf {
    sibling_bin("ene-ctl")
}

async fn spawn_core(dir: &TempDir) -> (KillOnDrop, ApiClient) {
    let child = Command::new(ene_core_bin())
        .arg("--data-dir")
        .arg(dir.path())
        .env("RUST_LOG", "error")
        .spawn()
        .expect("spawn ene-core");
    let child = KillOnDrop(Some(child));
    let ready = wait_for_api_json(&dir.path().join("api.json"), Duration::from_secs(20))
        .await
        .unwrap();
    let api = read_api_ready(&dir.path().join("api.json")).unwrap();
    assert_eq!(api.url, ready.url);
    let client = ApiClient::new(ready.url, ready.token.unwrap_or_default(), "cli");
    client.health().await.unwrap();
    (child, client)
}

#[tokio::test]
async fn ctl_client_lists_tools_and_debug_spans() {
    let dir = TempDir::new().unwrap();
    let (_child, client) = spawn_core(&dir).await;
    let tools = client.list_tools().await.unwrap();
    assert!(!tools.items.is_empty());
    let soul_id = client
        .list_souls()
        .await
        .unwrap()
        .items
        .first()
        .map(|soul| soul.id.clone())
        .expect("boot seeds souls");
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id,
            title: Some("ctl picnic".into()),
        })
        .await
        .unwrap();
    client
        .send_message(
            &session.id,
            &MessageRequest {
                text: "ctl hello pineapple".into(),
                mode: MessageMode::Prompt,
                input_modality: None,
            },
            None,
        )
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw_user = false;
    while std::time::Instant::now() < deadline {
        let history = client.history(&session.id, "surface").await.unwrap();
        if history
            .messages
            .iter()
            .any(|message| message.role == "user")
        {
            saw_user = true;
            tokio::time::sleep(Duration::from_millis(200)).await;
            let history = client.history(&session.id, "surface").await.unwrap();
            assert!(
                history
                    .messages
                    .iter()
                    .all(|message| message.role != "assistant"),
                "unconfigured chat must not emit Echo assistant replies"
            );
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(saw_user, "ctl chat never persisted the user line");
    let found = ene_ctl::session::search_sessions(&client, "pineapple")
        .await
        .unwrap();
    assert!(found.items.iter().any(|item| item.id == session.id));
    let split = ene_ctl::session::split_session(&client, &session.id)
        .await
        .unwrap();
    assert_eq!(split.previous.id, session.id);
    assert_ne!(split.session.id, session.id);
    let ended = client
        .end_session(
            &split.session.id,
            &EndSessionRequest {
                reason: "explicit".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(ended.end_reason.as_deref(), Some("explicit"));
    let spans = client.diag_spans().await.unwrap();
    assert!(!spans.items.is_empty());
}

#[tokio::test]
async fn cli_binary_starts_core_and_runs_session_ops() {
    let core_bin = ene_core_bin();
    let ctl_bin = ene_ctl_bin();
    assert!(
        core_bin.is_file(),
        "ene-core missing at {} (build ene-daemon first)",
        core_bin.display()
    );
    assert!(
        ctl_bin.is_file(),
        "ene-ctl missing at {}",
        ctl_bin.display()
    );
    let path_env = match core_bin.parent() {
        Some(dir) => match std::env::var("PATH") {
            Ok(path) => format!("{}:{path}", dir.display()),
            Err(_) => dir.display().to_string(),
        },
        None => std::env::var("PATH").unwrap_or_default(),
    };

    let dir = TempDir::new().unwrap();
    let start = Command::new(&ctl_bin)
        .args(["core", "start", "--data-dir"])
        .arg(dir.path())
        .env("PATH", &path_env)
        .env("RUST_LOG", "error")
        .output()
        .expect("core start");
    assert!(
        start.status.success(),
        "core start failed:\n{}",
        String::from_utf8_lossy(&start.stderr)
    );
    let ready = wait_for_api_json(&dir.path().join("api.json"), Duration::from_secs(20))
        .await
        .unwrap();
    let url = ready.url;
    let token = ready.token.unwrap_or_default();

    let run = |args: &[&str]| {
        Command::new(&ctl_bin)
            .args(args)
            .env("ENE_API_URL", &url)
            .env("ENE_API_TOKEN", &token)
            .env("RUST_LOG", "error")
            .output()
            .expect("run ene-ctl")
    };

    let status = run(&["status"]);
    assert!(status.status.success());
    assert!(String::from_utf8_lossy(&status.stdout).contains("ok"));

    let souls = run(&["soul", "list"]);
    assert!(souls.status.success());
    let soul_page: serde_json::Value = serde_json::from_slice(&souls.stdout).unwrap();
    let soul_id = soul_page["items"][0]["id"]
        .as_str()
        .expect("seeded soul")
        .to_owned();
    let create = run(&["session", "create", soul_id.as_str()]);
    assert!(
        create.status.success(),
        "session create failed:\n{}",
        String::from_utf8_lossy(&create.stderr)
    );
    let created: serde_json::Value = serde_json::from_slice(&create.stdout).unwrap();
    let session_id = created["id"].as_str().unwrap().to_owned();

    let chat = run(&["chat", session_id.as_str(), "ctl integration ping"]);
    assert!(chat.status.success());

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw_user = false;
    let mut saw_assistant = false;
    while std::time::Instant::now() < deadline {
        let log = run(&["debug", "log", session_id.as_str()]);
        let out = String::from_utf8_lossy(&log.stdout);
        if out.contains("user") {
            saw_user = true;
            tokio::time::sleep(Duration::from_millis(200)).await;
            let log = run(&["debug", "log", session_id.as_str()]);
            let out = String::from_utf8_lossy(&log.stdout);
            saw_assistant = out.contains("assistant");
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    assert!(saw_user, "CLI chat never produced a user line");
    assert!(
        !saw_assistant,
        "unconfigured CLI chat must not emit an Echo assistant line"
    );

    let search = run(&["session", "search", session_id.as_str()]);
    assert!(search.status.success());
    let search_page: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    assert!(
        search_page["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"].as_str() == Some(session_id.as_str()))
    );

    let split = run(&["session", "split", session_id.as_str()]);
    assert!(
        split.status.success(),
        "split failed:\n{}",
        String::from_utf8_lossy(&split.stderr)
    );
    let split_resp: serde_json::Value = serde_json::from_slice(&split.stdout).unwrap();
    let new_session_id = split_resp["session"]["id"].as_str().unwrap().to_owned();
    assert_ne!(new_session_id, session_id);

    let end = run(&["session", "end", new_session_id.as_str()]);
    assert!(end.status.success());
    let ended: serde_json::Value = serde_json::from_slice(&end.stdout).unwrap();
    assert_eq!(ended["end_reason"].as_str(), Some("explicit"));

    let tools = run(&["tool", "list"]);
    assert!(tools.status.success());

    let stop = Command::new(&ctl_bin)
        .args(["core", "stop", "--data-dir"])
        .arg(dir.path())
        .env("PATH", &path_env)
        .output()
        .expect("core stop");
    assert!(
        stop.status.success(),
        "core stop failed:\n{}",
        String::from_utf8_lossy(&stop.stderr)
    );
}
