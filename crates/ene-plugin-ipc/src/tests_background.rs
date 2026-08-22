use crate::{
    HostConn, HostHello, IpcError, ProtoId, ProtocolRanges, ToolBackgroundStart, ToolCall,
    ToolExecutionComplete, ToolHandler, ToolSpecWire, serve_plugin,
};
use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::DuplexStream;
use tokio::time::{Duration, sleep};

const SPAWN_TOKEN: &str = "test-spawn-token";

struct Live {
    cancel: AtomicBool,
    phase: Mutex<String>,
    child: Mutex<Option<std::process::Child>>,
}

struct BgHandler {
    lives: Arc<Mutex<HashMap<String, Arc<Live>>>>,
    completions: Arc<Mutex<Vec<ToolExecutionComplete>>>,
}

impl BgHandler {
    fn new() -> Self {
        Self {
            lives: Arc::new(Mutex::new(HashMap::new())),
            completions: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
#[expect(
    clippy::unnecessary_literal_bound,
    reason = "test stand-in returns fixed plugin identity strings"
)]
impl ToolHandler for BgHandler {
    fn plugin_id(&self) -> &str {
        "tool.bg"
    }
    fn plugin_name(&self) -> &str {
        "bg"
    }
    fn digest(&self) -> &str {
        "sha256:test"
    }
    fn specs(&self) -> Vec<ToolSpecWire> {
        vec![
            ToolSpecWire {
                name: "utility.hash".to_owned(),
                description: "hash text".to_owned(),
                parameters: json!({"type":"object"}),
                output: json!({"type":"object"}),
                side_effects: Vec::new(),
                broker_socket: None,
                category: String::new(),
                keywords: Vec::new(),
                examples: Vec::new(),
                background: false,
            },
            ToolSpecWire {
                name: "bg.sleep".to_owned(),
                description: "sleep then complete".to_owned(),
                parameters: json!({"type":"object"}),
                output: json!({"type":"object"}),
                side_effects: Vec::new(),
                broker_socket: None,
                category: String::new(),
                keywords: Vec::new(),
                examples: Vec::new(),
                background: true,
            },
        ]
    }
    async fn call(
        &self,
        name: &str,
        _args: serde_json::Value,
    ) -> Result<serde_json::Value, IpcError> {
        if name == "utility.hash" {
            return Ok(json!({ "ok": true }));
        }
        Err(IpcError::UnknownTool(name.to_owned()))
    }
    fn spawn_token(&self) -> Result<String, String> {
        Ok(SPAWN_TOKEN.to_owned())
    }
    async fn start_background(
        &self,
        execution_id: &str,
        name: &str,
        args: serde_json::Value,
        _deadline_ms: Option<u64>,
    ) -> Result<(), IpcError> {
        if name != "bg.sleep" {
            return Err(IpcError::UnknownTool(name.to_owned()));
        }
        let ms = args
            .get("ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(80);
        let live = Arc::new(Live {
            cancel: AtomicBool::new(false),
            phase: Mutex::new("running".to_owned()),
            child: Mutex::new(None),
        });
        #[cfg(unix)]
        {
            let child = std::process::Command::new("sleep").arg("30").spawn().ok();
            *live.child.lock().expect("child lock") = child;
        }
        self.lives
            .lock()
            .expect("lives lock")
            .insert(execution_id.to_owned(), Arc::clone(&live));
        let completions = Arc::clone(&self.completions);
        let execution_id = execution_id.to_owned();
        tokio::spawn(async move {
            let slice = Duration::from_millis(ms);
            let started = tokio::time::Instant::now();
            loop {
                if live.cancel.load(Ordering::SeqCst) {
                    kill_child(&live);
                    *live.phase.lock().expect("phase") = "cancelled".to_owned();
                    completions
                        .lock()
                        .expect("completions")
                        .push(ToolExecutionComplete {
                            execution_id,
                            call_id: "c-bg".to_owned(),
                            status: "cancelled".to_owned(),
                            value: json!({}),
                            error_class: None,
                        });
                    return;
                }
                if started.elapsed() >= slice {
                    kill_child(&live);
                    *live.phase.lock().expect("phase") = "completed".to_owned();
                    completions
                        .lock()
                        .expect("completions")
                        .push(ToolExecutionComplete {
                            execution_id,
                            call_id: "c-bg".to_owned(),
                            status: "ok".to_owned(),
                            value: json!({ "slept_ms": ms }),
                            error_class: None,
                        });
                    return;
                }
                sleep(Duration::from_millis(5)).await;
            }
        });
        Ok(())
    }
    fn cancel_background(&self, execution_id: &str) -> crate::ToolBackgroundCancelAck {
        let lives = self.lives.lock().expect("lives lock");
        let Some(live) = lives.get(execution_id) else {
            return crate::ToolBackgroundCancelAck {
                execution_id: execution_id.to_owned(),
                status: "unknown".to_owned(),
            };
        };
        let phase = live.phase.lock().expect("phase").clone();
        if matches!(phase.as_str(), "completed" | "cancelled" | "timed_out") {
            return crate::ToolBackgroundCancelAck {
                execution_id: execution_id.to_owned(),
                status: "already_terminal".to_owned(),
            };
        }
        live.cancel.store(true, Ordering::SeqCst);
        crate::ToolBackgroundCancelAck {
            execution_id: execution_id.to_owned(),
            status: "cancelled".to_owned(),
        }
    }
    fn status_background(&self, execution_id: &str) -> crate::ToolBackgroundStatusResult {
        let lives = self.lives.lock().expect("lives lock");
        let Some(live) = lives.get(execution_id) else {
            return crate::ToolBackgroundStatusResult {
                execution_id: execution_id.to_owned(),
                phase: "unknown".to_owned(),
                error_class: Some("unknown_execution".to_owned()),
            };
        };
        crate::ToolBackgroundStatusResult {
            execution_id: execution_id.to_owned(),
            phase: live.phase.lock().expect("phase").clone(),
            error_class: None,
        }
    }
    fn poll_background(&self) -> Vec<ToolExecutionComplete> {
        std::mem::take(&mut *self.completions.lock().expect("completions"))
    }
}

fn kill_child(live: &Live) {
    if let Some(mut child) = live.child.lock().expect("child").take() {
        drop(child.kill());
        drop(child.wait());
    }
}

fn hello() -> HostHello {
    HostHello {
        host_name: "ene-core".to_owned(),
        host_version: "0.1.0".to_owned(),
        protocols: ProtocolRanges::host_supported(),
        expected_digest: "sha256:test".to_owned(),
        declared_protocols: vec![ProtoId::Core, ProtoId::Tool],
        max_frame_bytes: 0,
        allow_unverified: false,
    }
}

fn spawn_bg(plugin_side: DuplexStream) -> tokio::task::JoinHandle<Result<(), IpcError>> {
    tokio::spawn(async move { serve_plugin(plugin_side, BgHandler::new()).await })
}

#[tokio::test]
async fn sync_tools_still_roundtrip_alongside_background_spec() {
    let (host_side, plugin_side) = tokio::io::duplex(4096);
    let plugin = spawn_bg(plugin_side);
    let mut host = HostConn::handshake(
        host_side,
        hello(),
        &[ProtoId::Core, ProtoId::Tool],
        SPAWN_TOKEN,
    )
    .await
    .unwrap();
    let specs = host.list_tools().await.unwrap();
    assert!(!specs[0].background);
    assert!(
        specs
            .iter()
            .any(|spec| spec.name == "bg.sleep" && spec.background)
    );
    let result = host
        .call_tool(ToolCall {
            call_id: "c1".to_owned(),
            tool_name: "utility.hash".to_owned(),
            args: json!({}),
            deadline_ms: None,
        })
        .await
        .unwrap();
    assert_eq!(result.status, "ok");
    host.drain().await.unwrap();
    plugin.await.unwrap().unwrap();
}

#[tokio::test]
async fn background_start_returns_id_and_pushes_completion() {
    let (host_side, plugin_side) = tokio::io::duplex(4096);
    let plugin = spawn_bg(plugin_side);
    let mut host = HostConn::handshake(
        host_side,
        hello(),
        &[ProtoId::Core, ProtoId::Tool],
        SPAWN_TOKEN,
    )
    .await
    .unwrap();
    let started = host
        .start_background(ToolBackgroundStart {
            call_id: "c-bg".to_owned(),
            tool_name: "bg.sleep".to_owned(),
            args: json!({"ms": 40}),
            execution_id: "exec-1".to_owned(),
            deadline_ms: None,
        })
        .await
        .unwrap();
    assert!(started.accepted);
    assert_eq!(started.execution_id, "exec-1");
    let mut complete = None;
    for _ in 0..50 {
        let _ = host.ping().await.unwrap();
        if let Some(body) = host.take_completion("exec-1") {
            complete = Some(body);
            break;
        }
        let status = host.status_background("exec-1").await.unwrap();
        if status.phase == "completed" {
            let _ = host.ping().await.unwrap();
            if let Some(body) = host.take_completion("exec-1") {
                complete = Some(body);
                break;
            }
        }
        sleep(Duration::from_millis(20)).await;
    }
    let complete = complete.expect("completion notification");
    assert_eq!(complete.execution_id, "exec-1");
    assert_eq!(complete.status, "ok");
    let again = host.status_background("exec-1").await.unwrap();
    assert_eq!(again.phase, "completed");
    host.drain().await.unwrap();
    plugin.await.unwrap().unwrap();
}

#[tokio::test]
async fn cancel_is_idempotent_and_unknown_id_is_distinct() {
    let (host_side, plugin_side) = tokio::io::duplex(4096);
    let plugin = spawn_bg(plugin_side);
    let mut host = HostConn::handshake(
        host_side,
        hello(),
        &[ProtoId::Core, ProtoId::Tool],
        SPAWN_TOKEN,
    )
    .await
    .unwrap();
    host.start_background(ToolBackgroundStart {
        call_id: "c-bg".to_owned(),
        tool_name: "bg.sleep".to_owned(),
        args: json!({"ms": 5_000}),
        execution_id: "exec-2".to_owned(),
        deadline_ms: None,
    })
    .await
    .unwrap();
    let first = host.cancel_background("exec-2").await.unwrap();
    assert_eq!(first.status, "cancelled");
    let second = host.cancel_background("exec-2").await.unwrap();
    assert!(
        second.status == "cancelled" || second.status == "already_terminal",
        "{}",
        second.status
    );
    let unknown = host.cancel_background("missing").await.unwrap();
    assert_eq!(unknown.status, "unknown");
    let status = host.status_background("missing").await.unwrap();
    assert_eq!(status.phase, "unknown");
    assert_eq!(status.error_class.as_deref(), Some("unknown_execution"));
    let refused = host
        .start_background(ToolBackgroundStart {
            call_id: "c-sync".to_owned(),
            tool_name: "utility.hash".to_owned(),
            args: json!({}),
            execution_id: "exec-sync".to_owned(),
            deadline_ms: None,
        })
        .await
        .unwrap();
    assert!(!refused.accepted);
    host.drain().await.unwrap();
    plugin.await.unwrap().unwrap();
}

#[tokio::test]
async fn take_completion_does_not_steal_other_executions() {
    let (host_side, plugin_side) = tokio::io::duplex(4096);
    let plugin = spawn_bg(plugin_side);
    let mut host = HostConn::handshake(
        host_side,
        hello(),
        &[ProtoId::Core, ProtoId::Tool],
        SPAWN_TOKEN,
    )
    .await
    .unwrap();
    host.start_background(ToolBackgroundStart {
        call_id: "c-a".to_owned(),
        tool_name: "bg.sleep".to_owned(),
        args: json!({"ms": 80}),
        execution_id: "exec-a".to_owned(),
        deadline_ms: None,
    })
    .await
    .unwrap();
    host.start_background(ToolBackgroundStart {
        call_id: "c-b".to_owned(),
        tool_name: "bg.sleep".to_owned(),
        args: json!({"ms": 10}),
        execution_id: "exec-b".to_owned(),
        deadline_ms: None,
    })
    .await
    .unwrap();
    let mut saw_b = false;
    for _ in 0..80 {
        let _ = host.ping().await.unwrap();
        if host.take_completion("exec-b").is_some() {
            saw_b = true;
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    assert!(saw_b, "exec-b should complete first");
    assert!(host.take_completion("exec-a").is_none());
    let mut saw_a = false;
    for _ in 0..80 {
        let _ = host.ping().await.unwrap();
        if host.take_completion("exec-a").is_some() {
            saw_a = true;
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    assert!(saw_a, "exec-a should complete after exec-b");
    host.drain().await.unwrap();
    plugin.await.unwrap().unwrap();
}
