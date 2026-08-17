//! W7: spawn the `ene-core` binary (`EchoModel` / offline) and record `minimal` baselines.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "integration tests fail fast"
)]
#![deny(unsafe_code)]

use ene_api::{ApiClient, CreateSessionRequest, MessageMode, MessageRequest};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct KillOnDrop(Option<Child>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            if child.kill().is_err() {
                // Process already exited.
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

fn rss_kb(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

async fn wait_ready(path: &Path) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(value) = serde_json::from_str::<Value>(&text)
            && value.get("url").and_then(Value::as_str).is_some()
        {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "ene-core did not write api.json at {}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn spawned_core_offline_conversation_and_rss() {
    let dir = TempDir::new().unwrap();
    let started = Instant::now();
    let child = Command::new(ene_core_bin())
        .arg("--data-dir")
        .arg(dir.path())
        .env("RUST_LOG", "error")
        .spawn()
        .expect("spawn ene-core");
    let pid = child.id();
    let child = KillOnDrop(Some(child));
    let ready = wait_ready(&dir.path().join("api.json")).await;
    let url = ready["url"].as_str().unwrap().to_owned();
    let token_path = dir.path().join(
        ready
            .get("token_file")
            .and_then(Value::as_str)
            .unwrap_or("api.token"),
    );
    let token = std::fs::read_to_string(&token_path)
        .expect("api.token")
        .trim()
        .to_owned();
    let client = ApiClient::new(url, token, "cli");
    let mut health_ok = false;
    let health_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < health_deadline {
        if client.health().await.is_ok() {
            health_ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(health_ok, "health never succeeded");
    let boot_ms = started.elapsed().as_millis();
    let rss = rss_kb(pid);
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
            title: None,
        })
        .await
        .unwrap();
    client
        .send_message(
            &session.id,
            &MessageRequest {
                text: "offline hello".into(),
                mode: MessageMode::Prompt,
                input_modality: None,
            },
            None,
        )
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_assistant = false;
    while Instant::now() < deadline {
        let history = client.history(&session.id, "surface").await.unwrap();
        if history
            .messages
            .iter()
            .any(|message| message.role == "assistant")
        {
            saw_assistant = true;
            assert!(
                history
                    .messages
                    .iter()
                    .all(|message| message.role != "inner")
            );
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(saw_assistant, "offline EchoModel conversation failed");
    let detail = client.history(&session.id, "detail").await.unwrap();
    assert!(
        detail
            .messages
            .iter()
            .any(|message| message.role == "inner")
    );
    std::fs::create_dir_all("/opt/cursor/artifacts").unwrap();
    std::fs::write(
        "/opt/cursor/artifacts/minimal_process_baseline.txt",
        format!(
            "boot_to_health_ms={boot_ms} rss_kb={}\n",
            rss.map_or_else(|| "unknown".to_owned(), |kb| kb.to_string())
        ),
    )
    .unwrap();
    assert!(boot_ms < 8_000, "boot_to_health_ms={boot_ms}");
    if let Some(kb) = rss {
        assert!(kb < 512_000, "idle rss_kb={kb}");
    }
    drop(child);
}
