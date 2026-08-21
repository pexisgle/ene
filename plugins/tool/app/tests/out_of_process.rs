#![expect(clippy::unwrap_used, reason = "tests fail fast")]

use ene_fiber::{ProfileRow, Supervisor};
use ene_registry::{Layer, ToolRegistry};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ene-tool-app"))
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
        seams: Vec::new(),
        sandbox_required: false,
        config: serde_json::Value::Null,
    };
    sup.activate_process(&row, &bin()).await.unwrap();
    assert!(sup.surface_has_tool("app.screenshot"));
    assert!(sup.surface_has_tool("app.capabilities"));
    assert!(
        !sup.surface_has_tool("app.click"),
        "input injection stays off the surface schema"
    );
    let caps = sup
        .registry()
        .execute("app.capabilities", json!({}), Layer::Surface)
        .await
        .unwrap();
    assert!(caps["session"].is_string());
    assert!(caps["actions"]["app.screenshot"]["backend"].is_string());
    assert!(caps["actions"]["app.click"].get("available").is_some());
    sup.unload("r-app").await;
}

#[test]
fn plugin_serves_local_logic_not_builtin_kind() {
    let src = include_str!("../src/main.rs");
    assert!(!src.contains("BuiltinKind"));
    assert!(src.contains("run_tool_plugin"));
}
