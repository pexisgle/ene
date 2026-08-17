#![expect(clippy::unwrap_used, reason = "tests fail fast")]

use ene_fiber::{ProfileRow, Supervisor};
use ene_registry::{Layer, PipelineError, ToolRegistry};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ene-harness-fs"))
}

#[tokio::test]
async fn fs_write_is_denied_and_absent_from_surface() {
    let dir = TempDir::new().unwrap();
    let allowed = dir.path().join("workspace");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::write(allowed.join("ok.txt"), "visible").unwrap();
    let sup = Supervisor::new(allowed.clone(), Arc::new(ToolRegistry::new()));
    let row = ProfileRow {
        row_id: "r-fs".to_owned(),
        plugin: "tool.fs".to_owned(),
        requires: Vec::new(),
        capabilities: vec!["fs.read".to_owned()],
        sandbox_required: ene_sandbox::supported(),
    };
    sup.activate_process(&row, &bin()).await.unwrap();
    let surface = sup.registry().schemas(Layer::Surface);
    let names: Vec<&str> = surface
        .iter()
        .filter_map(|schema| schema.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(names.contains(&"fs.read"));
    assert!(!names.contains(&"fs.write"));
    let denied = sup
        .registry()
        .execute(
            "fs.write",
            json!({"path": allowed.join("no.txt").to_string_lossy(), "text": "x"}),
            Layer::Job,
        )
        .await
        .unwrap_err();
    assert!(matches!(denied, PipelineError::Denied { .. }));
    let read = sup
        .registry()
        .execute(
            "fs.read",
            json!({"path": allowed.join("ok.txt").to_string_lossy()}),
            Layer::Surface,
        )
        .await
        .unwrap();
    assert_eq!(read["text"], "visible");
    if ene_sandbox::supported() {
        let secret_dir = dir.path().join("secret");
        std::fs::create_dir_all(&secret_dir).unwrap();
        let secret = secret_dir.join("hidden.txt");
        std::fs::write(&secret, "hidden").unwrap();
        let err = sup
            .registry()
            .execute(
                "fs.read",
                json!({"path": secret.to_string_lossy()}),
                Layer::Surface,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, PipelineError::Execute(_)));
    }
    sup.unload("r-fs").await;
}
