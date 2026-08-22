#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    reason = "tests fail fast"
)]

use ene_fiber::{ProfileRow, Supervisor};
use ene_registry::{Layer, ToolRegistry};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ene-tool-utility"))
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
async fn every_advertised_utility_spec_executes_on_spawned_binary() {
    let dir = TempDir::new().unwrap();
    let sup = Supervisor::new(dir.path().to_path_buf(), Arc::new(ToolRegistry::new()));
    let uid = sup
        .activate_process(&row("r-util-contract"), &bin())
        .await
        .unwrap();
    let names: Vec<String> = sup
        .registry()
        .schemas(Layer::Surface)
        .iter()
        .filter_map(|schema| schema.get("name").and_then(serde_json::Value::as_str))
        .filter_map(|name| name.strip_prefix("utility.").map(str::to_owned))
        .collect();
    assert_eq!(
        names,
        vec![
            "calc",
            "color",
            "hash",
            "random",
            "system_info",
            "text",
            "time"
        ]
    );
    for name in [
        "hash",
        "time",
        "system_info",
        "calc",
        "color",
        "random",
        "text",
    ] {
        let result = sup
            .registry()
            .execute(
                &format!("utility.{name}"),
                sample_args(name),
                Layer::Surface,
            )
            .await;
        assert!(result.is_ok(), "utility.{name} failed: {result:?}");
    }
    let _ = uid;
    sup.unload("r-util-contract").await;
}

fn sample_args(action: &str) -> serde_json::Value {
    match action {
        "hash" => json!({"text": "hi"}),
        "time" | "system_info" => json!({}),
        "calc" => json!({"expr": "1 + 2"}),
        "color" => json!({"color": "#ff8800", "to": "rgb"}),
        "random" => json!({"kind": "integer", "min": 0, "max": 0}),
        "text" => json!({"op": "encode", "text": "hi", "encoding": "base64"}),
        other => unreachable!("missing contract args for {other}"),
    }
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
