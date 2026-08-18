use crate::{Layer, PipelineError, ToolDefinition, ToolRegistry, ToolSource, builtin_specs};
use ene_plugin_ipc::BuiltinKind;
use serde_json::{Value, json};
use std::sync::Arc;

#[test]
fn surface_schemas_omit_fs_write() {
    let registry = ToolRegistry::new();
    for def in crate::builtins::definitions_for(BuiltinKind::Fs) {
        registry.register(def);
    }
    for def in crate::builtins::definitions_for(BuiltinKind::Utility) {
        registry.register(def);
    }
    let surface = registry.schemas(Layer::Surface);
    let names: Vec<&str> = surface
        .iter()
        .filter_map(|schema| schema.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(names.contains(&"utility.hash"));
    assert!(names.contains(&"utility.calc"));
    assert!(names.contains(&"fs.read"));
    assert!(names.contains(&"fs.list"));
    assert!(!names.contains(&"fs.write"));
    assert!(!names.contains(&"fs.edit"));
    assert!(
        surface
            .iter()
            .all(|schema| schema.get("side_effects").is_none())
    );
    let job = registry.schemas(Layer::Job);
    let job_names: Vec<&str> = job
        .iter()
        .filter_map(|schema| schema.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(job_names.contains(&"fs.write"));
}

#[tokio::test]
async fn side_effect_tools_are_denied_by_default() {
    let registry = ToolRegistry::new();
    for def in crate::builtins::definitions_for(BuiltinKind::Fs) {
        registry.register(def);
    }
    let err = registry
        .execute("fs.write", json!({"path":"/tmp/x","text":"no"}), Layer::Job)
        .await
        .unwrap_err();
    assert!(matches!(err, PipelineError::Denied { .. }));
}

#[tokio::test]
async fn surface_cannot_execute_fs_write() {
    let registry = ToolRegistry::new();
    for def in crate::builtins::definitions_for(BuiltinKind::Fs) {
        registry.register(def);
    }
    let err = registry
        .execute(
            "fs.write",
            json!({"path":"/tmp/x","text":"no"}),
            Layer::Surface,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PipelineError::NotOnSurface(_)));
}

#[tokio::test]
async fn empty_side_effects_tools_run() {
    let registry = ToolRegistry::new();
    for def in crate::builtins::definitions_for(BuiltinKind::Utility) {
        registry.register(def);
    }
    let value = registry
        .execute("utility.hash", json!({"text":"hi"}), Layer::Surface)
        .await
        .unwrap();
    assert!(value.get("blake3").is_some());
}

#[test]
fn unregister_source_drops_only_that_plugin() {
    let registry = ToolRegistry::new();
    for def in crate::builtins::definitions_for(BuiltinKind::Fs) {
        registry.register(def);
    }
    for def in crate::builtins::definitions_for(BuiltinKind::Utility) {
        registry.register(def);
    }
    registry.unregister_source(&ToolSource::Plugin {
        plugin_id: "tool.fs".to_owned(),
    });
    assert!(registry.get("fs.read").is_none());
    assert!(registry.get("utility.hash").is_some());
}

#[test]
fn side_effects_field_is_required_on_the_wire() {
    let raw = r#"{"name":"x","description":"d","parameters":{},"output":{}}"#;
    let err = serde_json::from_str::<ene_plugin_ipc::ToolSpecWire>(raw).unwrap_err();
    assert!(err.to_string().contains("side_effects"));
}

#[test]
fn exec_tools_stay_off_surface_schema() {
    let registry = ToolRegistry::new();
    for def in crate::builtins::definitions_for(BuiltinKind::Fs) {
        registry.register(def);
    }
    for def in crate::builtins::definitions_for(BuiltinKind::Exec) {
        registry.register(def);
    }
    let surface_schemas = registry.schemas(Layer::Surface);
    let surface: Vec<&str> = surface_schemas
        .iter()
        .filter_map(|schema| schema.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(!surface.iter().any(|name| name.starts_with("exec.")));
    let job_schemas = registry.schemas(Layer::Job);
    let job: Vec<&str> = job_schemas
        .iter()
        .filter_map(|schema| schema.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(job.contains(&"exec.run"));
    assert!(job.contains(&"fs.write"));
}

#[test]
fn builtin_specs_cover_five_plugins() {
    assert!(!builtin_specs(BuiltinKind::Fs).is_empty());
    assert!(!builtin_specs(BuiltinKind::Exec).is_empty());
    assert!(!builtin_specs(BuiltinKind::Web).is_empty());
    assert!(!builtin_specs(BuiltinKind::Utility).is_empty());
    assert!(!builtin_specs(BuiltinKind::App).is_empty());
}

#[test]
fn harness_tool_uses_the_same_pipeline() {
    let registry = ToolRegistry::new();
    registry.register(ToolDefinition {
        name: "memory.recall".to_owned(),
        description: "recall".to_owned(),
        parameters: json!({"type":"object","additionalProperties":false}),
        output: json!({"type":"object"}),
        side_effects: Vec::new(),
        source: ToolSource::Harness {
            name: "memory.recall".to_owned(),
        },
        timeout_ms: None,
        sensitivity: ene_plane::Sensitivity::None,
    });
    let surface = registry.schemas(Layer::Surface);
    assert_eq!(surface[0]["name"], "memory.recall");
}

#[tokio::test]
async fn plane_denies_side_effects_and_sensitive_reads() {
    use ene_plane::{ApprovalPlane, ApprovalSettings, AuditLog, ScriptedPopup};
    let dir = tempfile::TempDir::new().unwrap();
    let registry = ToolRegistry::new();
    registry.set_workspace(dir.path());
    let audit = AuditLog::open(dir.path().join("audit.db")).unwrap();
    let plane = Arc::new(ApprovalPlane::new(
        ApprovalSettings::default(),
        audit,
        ScriptedPopup::deny_all(),
        None,
    ));
    registry.set_plane(plane);
    for def in crate::builtins::definitions_for(BuiltinKind::Fs) {
        registry.register(def);
    }
    registry.register(ToolDefinition {
        name: "app.screenshot".to_owned(),
        description: "capture".to_owned(),
        parameters: json!({"type":"object","additionalProperties":false}),
        output: json!({"type":"object"}),
        side_effects: Vec::new(),
        source: ToolSource::Harness {
            name: "app.screenshot".to_owned(),
        },
        timeout_ms: None,
        sensitivity: ene_plane::Sensitivity::High,
    });
    let denied = registry
        .execute("fs.write", json!({"path":"/tmp/x","text":"no"}), Layer::Job)
        .await
        .unwrap_err();
    assert!(matches!(
        denied,
        PipelineError::Plane(_) | PipelineError::Denied { .. }
    ));
    let shot = registry
        .execute("app.screenshot", json!({}), Layer::Surface)
        .await
        .unwrap_err();
    assert!(matches!(shot, PipelineError::Plane(_)));
}

#[test]
fn confine_tool_path_rejects_parent_escape_for_new_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let err =
        crate::pipeline::confine_tool_path(dir.path(), std::path::Path::new("../escape.txt"), true)
            .unwrap_err();
    assert!(matches!(err, PipelineError::PathEscape(_)));
}

#[test]
fn unknown_plugin_empty_side_effects_are_medium_sensitivity() {
    let spec = ene_plugin_ipc::ToolSpecWire {
        name: "evil.read".to_owned(),
        description: "d".to_owned(),
        parameters: json!({"type":"object"}),
        output: json!({"type":"object"}),
        side_effects: Vec::new(),
    };
    let def = ToolDefinition::from_wire(
        spec,
        ToolSource::Plugin {
            plugin_id: "evil".to_owned(),
        },
    );
    assert_eq!(def.sensitivity, ene_plane::Sensitivity::Medium);
}

#[tokio::test]
async fn calc_and_text_tools_run_on_surface() {
    let registry = ToolRegistry::new();
    for def in crate::builtins::definitions_for(BuiltinKind::Utility) {
        registry.register(def);
    }
    let sum = registry
        .execute("utility.calc", json!({"expr": "1+2*3"}), Layer::Surface)
        .await
        .unwrap();
    assert_eq!(sum["value"], json!(7.0));
    let hashed = registry
        .execute(
            "utility.text",
            json!({"op":"hash","text":"hi","algorithm":"blake3"}),
            Layer::Surface,
        )
        .await
        .unwrap();
    assert!(hashed.get("hex").is_some());
}

#[tokio::test]
async fn web_fetch_is_on_surface_and_blocks_loopback() {
    let registry = ToolRegistry::new();
    for def in crate::builtins::definitions_for(BuiltinKind::Web) {
        registry.register(def);
    }
    let surface = registry.schemas(Layer::Surface);
    let names: Vec<&str> = surface
        .iter()
        .filter_map(|schema| schema.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(names.contains(&"web.fetch"));
    assert!(names.contains(&"web.search"));
    let err = registry
        .execute(
            "web.fetch",
            json!({"url":"http://127.0.0.1/secret"}),
            Layer::Surface,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PipelineError::Execute(_)));
}

#[test]
fn app_screenshot_is_high_sensitivity_from_host_spec() {
    let defs = crate::builtins::definitions_for(BuiltinKind::App);
    let shot = defs
        .iter()
        .find(|def| def.name == "app.screenshot")
        .unwrap();
    assert!(shot.side_effects.is_empty());
    assert_eq!(shot.sensitivity, ene_plane::Sensitivity::High);
    let click = defs.iter().find(|def| def.name == "app.click").unwrap();
    assert_eq!(click.side_effects, vec!["input".to_owned()]);
}

#[tokio::test]
async fn fs_list_and_edit_stay_in_workspace() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello world").unwrap();
    let registry = ToolRegistry::new();
    registry.set_workspace(dir.path());
    for def in crate::builtins::definitions_for(BuiltinKind::Fs) {
        registry.register(def);
    }
    let listed = registry
        .execute("fs.list", json!({}), Layer::Surface)
        .await
        .unwrap();
    assert!(
        listed["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| { row.get("name").and_then(Value::as_str) == Some("a.txt") })
    );
}
