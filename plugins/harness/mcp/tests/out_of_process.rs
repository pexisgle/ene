#![expect(clippy::unwrap_used, reason = "tests fail fast")]

use ene_fiber::{ProfileRow, Supervisor};
use ene_registry::{Layer, ToolRegistry};
use serde_json::json;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ene-harness-mcp"))
}

fn fixture_script(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("mcp_fixture.py");
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(
        br#"#!/usr/bin/env python3
import json, sys

def read_msg():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            raise SystemExit(0)
        if line in (b"\r\n", b"\n"):
            break
        key, _, value = line.decode().partition(":")
        headers[key.strip().lower()] = value.strip()
    n = int(headers.get("content-length", "0"))
    return json.loads(sys.stdin.buffer.read(n))

def write_msg(obj):
    body = json.dumps(obj).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode())
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    msg = read_msg()
    method = msg.get("method")
    ident = msg.get("id")
    if method == "initialize":
        write_msg({"jsonrpc":"2.0","id":ident,"result":{
            "protocolVersion":"2024-11-05",
            "capabilities":{"tools":{}},
            "serverInfo":{"name":"fixture","version":"0"}
        }})
    elif method == "notifications/initialized":
        pass
    elif method == "tools/list":
        write_msg({"jsonrpc":"2.0","id":ident,"result":{"tools":[{
            "name":"ping",
            "description":"ping",
            "inputSchema":{"type":"object","additionalProperties":False}
        }]}})
    elif method == "tools/call":
        write_msg({"jsonrpc":"2.0","id":ident,"result":{
            "content":[{"type":"text","text":"pong"}]
        }})
"#,
    )
    .unwrap();
    path
}

#[tokio::test]
async fn handwritten_stdio_mcp_registers_and_runs() {
    let dir = TempDir::new().unwrap();
    let script = fixture_script(dir.path());
    let sup = Supervisor::new(dir.path().to_path_buf(), Arc::new(ToolRegistry::new()));
    let row = ProfileRow {
        row_id: "mcp.fixture".to_owned(),
        plugin: "mcp.fixture".to_owned(),
        requires: Vec::new(),
        capabilities: Vec::new(),
        sandbox_required: false,
        config: json!({
            "server": "fixture",
            "command": "python3",
            "args": [script.to_string_lossy()],
        }),
    };
    sup.activate_process(&row, &bin()).await.unwrap();
    assert!(sup.surface_has_tool("mcp:fixture.ping"));
    let value = sup
        .registry()
        .execute("mcp:fixture.ping", json!({}), Layer::Surface)
        .await
        .unwrap();
    assert!(value.to_string().contains("pong"));
    sup.unload("mcp.fixture").await;
}
