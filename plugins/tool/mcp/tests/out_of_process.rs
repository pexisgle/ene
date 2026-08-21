#![expect(clippy::unwrap_used, reason = "tests fail fast")]

use ene_fiber::{ProfileRow, Supervisor};
use ene_registry::{Layer, ToolRegistry};
use serde_json::json;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ene-tool-mcp"))
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
    elif method == "resources/list":
        write_msg({"jsonrpc":"2.0","id":ident,"result":{"resources":[{
            "uri":"memo://note",
            "name":"note"
        }]}})
    elif method == "resources/read":
        write_msg({"jsonrpc":"2.0","id":ident,"result":{"contents":[{
            "uri":"memo://note",
            "text":"hello resource"
        }]}})
    elif method == "prompts/list":
        write_msg({"jsonrpc":"2.0","id":ident,"result":{"prompts":[{
            "name":"brief",
            "description":"a brief"
        }]}})
    elif method == "prompts/get":
        write_msg({"jsonrpc":"2.0","id":ident,"result":{"messages":[{
            "role":"user",
            "content":{"type":"text","text":"do the brief"}
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

fn python3_bin() -> Option<PathBuf> {
    let output = std::process::Command::new("sh")
        .args(["-c", "command -v python3"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

#[tokio::test]
async fn handwritten_stdio_mcp_registers_and_runs() {
    if python3_bin().is_none() {
        return;
    }
    let dir = TempDir::new().unwrap();
    let script = fixture_script(dir.path());
    let sup = Supervisor::new(dir.path().to_path_buf(), Arc::new(ToolRegistry::new()));
    let row = ProfileRow {
        row_id: "mcp.fixture".to_owned(),
        plugin: "mcp.fixture".to_owned(),
        requires: Vec::new(),
        capabilities: Vec::new(),
        seams: Vec::new(),
        sandbox_required: false,
        config: json!({
            "server": "fixture",
            "command": "python3",
            "args": [script.to_string_lossy()],
            "skills_home": dir.path().join("skills"),
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
    let context = std::fs::read_to_string(dir.path().join("mcp-context/fixture.md")).unwrap();
    assert!(context.contains("hello resource"));
    let skill = std::fs::read_to_string(dir.path().join("skills/brief/SKILL.md")).unwrap();
    assert!(skill.contains("do the brief"));
    sup.unload("mcp.fixture").await;
}

fn git_bin() -> Option<PathBuf> {
    let output = std::process::Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

fn git_fixture_script(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("mcp_git.py");
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(
        br#"#!/usr/bin/env python3
import json, subprocess, sys

repo = sys.argv[1]

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

def git(*args):
    return subprocess.check_output(["git", "-C", repo, *args], text=True)

while True:
    msg = read_msg()
    method = msg.get("method")
    ident = msg.get("id")
    if method == "initialize":
        write_msg({"jsonrpc":"2.0","id":ident,"result":{
            "protocolVersion":"2024-11-05",
            "capabilities":{"tools":{}},
            "serverInfo":{"name":"git","version":"0"}
        }})
    elif method == "notifications/initialized":
        pass
    elif method == "tools/list":
        write_msg({"jsonrpc":"2.0","id":ident,"result":{"tools":[{
            "name":"status",
            "description":"git status --porcelain -b",
            "inputSchema":{"type":"object","additionalProperties":False}
        },{
            "name":"log",
            "description":"git log -1 --oneline",
            "inputSchema":{"type":"object","additionalProperties":False}
        }]}})
    elif method == "tools/call":
        name = (msg.get("params") or {}).get("name")
        try:
            if name == "status":
                text = git("status", "--porcelain", "-b")
            elif name == "log":
                text = git("log", "-1", "--oneline")
            else:
                raise ValueError(name)
            write_msg({"jsonrpc":"2.0","id":ident,"result":{
                "content":[{"type":"text","text": text}]
            }})
        except Exception as err:
            write_msg({"jsonrpc":"2.0","id":ident,"error":{"code":-32000,"message":str(err)}})
    elif method in ("resources/list", "prompts/list"):
        key = "resources" if method == "resources/list" else "prompts"
        write_msg({"jsonrpc":"2.0","id":ident,"result":{key:[]}})
"#,
    )
    .unwrap();
    path
}

fn init_git_repo(repo: &std::path::Path) {
    let git = git_bin().unwrap();
    let run = |args: &[&str]| {
        let status = std::process::Command::new(&git)
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "ene")
            .env("GIT_AUTHOR_EMAIL", "ene@example.invalid")
            .env("GIT_COMMITTER_NAME", "ene")
            .env("GIT_COMMITTER_EMAIL", "ene@example.invalid")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    run(&["init", "-q"]);
    run(&["config", "user.name", "ene"]);
    run(&["config", "user.email", "ene@example.invalid"]);
    run(&["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("README"), "hello from mcp git\n").unwrap();
    run(&["add", "README"]);
    run(&["commit", "-q", "-m", "init"]);
}

#[tokio::test]
async fn handwritten_stdio_mcp_git_status_runs_real_git() {
    if python3_bin().is_none() || git_bin().is_none() {
        return;
    }
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let script = git_fixture_script(dir.path());
    let sup = Supervisor::new(dir.path().to_path_buf(), Arc::new(ToolRegistry::new()));
    let row = ProfileRow {
        row_id: "mcp.git".to_owned(),
        plugin: "mcp.git".to_owned(),
        requires: Vec::new(),
        capabilities: Vec::new(),
        seams: Vec::new(),
        sandbox_required: false,
        config: json!({
            "server": "git",
            "command": "python3",
            "args": [script.to_string_lossy(), repo.to_string_lossy()],
        }),
    };
    sup.activate_process(&row, &bin()).await.unwrap();
    assert!(sup.surface_has_tool("mcp:git.status"));
    assert!(sup.surface_has_tool("mcp:git.log"));
    let status = sup
        .registry()
        .execute("mcp:git.status", json!({}), Layer::Surface)
        .await
        .unwrap();
    assert!(status.to_string().contains("## "), "status={status}");
    let log = sup
        .registry()
        .execute("mcp:git.log", json!({}), Layer::Surface)
        .await
        .unwrap();
    assert!(log.to_string().contains("init"), "log={log}");
    std::fs::write(repo.join("dirty.txt"), "unstaged\n").unwrap();
    let dirty = sup
        .registry()
        .execute("mcp:git.status", json!({}), Layer::Surface)
        .await
        .unwrap();
    assert!(dirty.to_string().contains("dirty.txt"), "dirty={dirty}");
    sup.unload("mcp.git").await;
}
