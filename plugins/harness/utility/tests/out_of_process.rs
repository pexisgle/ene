#![expect(clippy::unwrap_used, clippy::expect_used, reason = "tests fail fast")]

use ene_fiber::{ProfileRow, Supervisor};
use ene_registry::{Layer, ToolRegistry};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ene-harness-utility"))
}

fn row(id: &str) -> ProfileRow {
    ProfileRow {
        row_id: id.to_owned(),
        plugin: "tool.utility".to_owned(),
        requires: Vec::new(),
        capabilities: Vec::new(),
        seams: Vec::new(),
        sandbox_required: false,
        config: serde_json::Value::Null,
    }
}

#[tokio::test]
async fn out_of_process_utility_registers_and_runs() {
    let dir = TempDir::new().unwrap();
    let sup = Supervisor::new(dir.path().to_path_buf(), Arc::new(ToolRegistry::new()));
    let uid = sup.activate_process(&row("r-util"), &bin()).await.unwrap();
    assert!(!uid.to_string().is_empty());
    assert!(sup.surface_has_tool("utility.hash"));
    assert!(!sup.surface_has_tool("fs.write"));
    let value = sup
        .registry()
        .execute("utility.hash", json!({"text": "hi"}), Layer::Surface)
        .await
        .unwrap();
    assert!(value.get("hex").is_some());
    assert_eq!(
        value.get("algorithm").and_then(serde_json::Value::as_str),
        Some("blake3")
    );
    sup.unload("r-util").await;
    assert!(!sup.surface_has_tool("utility.hash"));
    assert!(sup.fiber("r-util").is_none());
}

#[tokio::test]
async fn process_survives_os_sandbox_when_supported() {
    if !ene_sandbox::supported() {
        return;
    }
    let dir = TempDir::new().unwrap();
    let sup = Supervisor::new(dir.path().to_path_buf(), Arc::new(ToolRegistry::new()));
    let mut row = row("r-sandboxed");
    row.sandbox_required = true;
    let uid = sup
        .activate_process(&row, &bin())
        .await
        .expect("utility must exec under Landlock");
    assert!(!uid.to_string().is_empty());
    let value = sup
        .registry()
        .execute("utility.hash", json!({"text": "sandboxed"}), Layer::Surface)
        .await
        .unwrap();
    assert!(value.get("hex").is_some());
    assert_eq!(
        value.get("algorithm").and_then(serde_json::Value::as_str),
        Some("blake3")
    );
    sup.unload("r-sandboxed").await;
}

#[test]
fn plugin_serves_local_logic_not_builtin_kind() {
    let src = include_str!("../src/main.rs");
    assert!(!src.contains("BuiltinKind"));
    assert!(src.contains("run_tool_plugin"));
}
