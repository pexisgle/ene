use crate::{Layer, PipelineError, ToolDefinition, ToolRegistry, ToolSource, builtin_specs};
use ene_plugin_ipc::BuiltinKind;
use serde_json::json;

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
    assert!(names.contains(&"fs.read"));
    assert!(!names.contains(&"fs.write"));
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
fn builtin_specs_cover_four_plugins() {
    assert!(!builtin_specs(BuiltinKind::Fs).is_empty());
    assert!(!builtin_specs(BuiltinKind::Exec).is_empty());
    assert!(!builtin_specs(BuiltinKind::Web).is_empty());
    assert!(!builtin_specs(BuiltinKind::Utility).is_empty());
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
    });
    let surface = registry.schemas(Layer::Surface);
    assert_eq!(surface[0]["name"], "memory.recall");
}
