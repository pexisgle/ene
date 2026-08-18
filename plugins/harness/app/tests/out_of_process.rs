#![expect(clippy::unwrap_used, reason = "tests fail fast")]

use ene_fiber::{ProfileRow, Supervisor};
use ene_plugin_ipc::BuiltinKind;
use ene_registry::{Layer, ToolRegistry};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ene-harness-app"))
}

#[tokio::test]
async fn app_screenshot_registers_on_surface() {
    let dir = TempDir::new().unwrap();
    let sup = Supervisor::new(dir.path().to_path_buf(), Arc::new(ToolRegistry::new()));
    let row = ProfileRow {
        row_id: "r-app".to_owned(),
        plugin: "tool.app".to_owned(),
        requires: Vec::new(),
        capabilities: Vec::new(),
        sandbox_required: false,
        config: serde_json::Value::Null,
    };
    sup.activate_process(&row, &bin()).await.unwrap();
    assert!(sup.surface_has_tool("app.screenshot"));
    assert!(!sup.surface_has_tool("app.click"));
    let listed = sup.registry().get("app.window_list").unwrap();
    assert_eq!(listed.sensitivity, ene_plane::Sensitivity::High);
    let err = sup
        .registry()
        .execute("app.click", json!({"x":0,"y":0}), Layer::Job)
        .await
        .unwrap_err();
    assert!(matches!(err, ene_registry::PipelineError::Denied { .. }));
    sup.unload("r-app").await;
}

#[test]
fn builtin_kind_matches_binary() {
    assert_eq!(BuiltinKind::App.plugin_id(), "tool.app");
}
