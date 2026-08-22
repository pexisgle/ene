use crate::{
    Layer, PipelineError, ToolDefinition, ToolInvoke, ToolRegistry, ToolSource, builtin_specs,
};
use async_trait::async_trait;
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
    assert!(names.contains(&"utility.system_info"));
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
    assert!(
        value.get("hex").is_some(),
        "utility.hash returns {{algorithm, hex}}, got {value}"
    );
    assert_eq!(
        value.get("algorithm").and_then(serde_json::Value::as_str),
        Some("blake3")
    );
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
fn unregister_drops_one_tool_and_keeps_siblings() {
    let registry = ToolRegistry::new();
    for def in crate::builtins::definitions_for(BuiltinKind::Utility) {
        registry.register(def);
    }
    registry.unregister("utility.hash");
    assert!(registry.get("utility.hash").is_none());
    assert!(registry.get("utility.time").is_some());
}

struct MarkerInvoke(&'static str);

#[async_trait]
impl ToolInvoke for MarkerInvoke {
    async fn invoke(&self, _name: &str, _args: Value) -> Result<Value, String> {
        Ok(json!({ "owner": self.0 }))
    }
}

fn overlapping_def() -> ToolDefinition {
    ToolDefinition {
        name: "overlap.tool".to_owned(),
        description: "d".to_owned(),
        parameters: json!({"type": "object"}),
        output: json!({"type": "object"}),
        side_effects: Vec::new(),
        source: ToolSource::Harness {
            name: "overlap.tool".to_owned(),
        },
        timeout_ms: None,
        sensitivity: ene_plane::Sensitivity::None,
        category: String::new(),
        keywords: Vec::new(),
        examples: Vec::new(),
        background: false,
    }
}

#[tokio::test]
async fn unregister_owned_keeps_later_owner() {
    let registry = ToolRegistry::new();
    registry.register_owned("fiber-a", overlapping_def(), Arc::new(MarkerInvoke("a")));
    registry.register_owned("fiber-b", overlapping_def(), Arc::new(MarkerInvoke("b")));
    registry.unregister_owned("overlap.tool", "fiber-a");
    let value = registry
        .execute("overlap.tool", json!({}), Layer::Surface)
        .await
        .unwrap();
    assert_eq!(value["owner"], json!("b"));
}

#[tokio::test]
async fn unregister_owned_restores_earlier_owner() {
    let registry = ToolRegistry::new();
    registry.register_owned("fiber-a", overlapping_def(), Arc::new(MarkerInvoke("a")));
    registry.register_owned("fiber-b", overlapping_def(), Arc::new(MarkerInvoke("b")));
    registry.unregister_owned("overlap.tool", "fiber-b");
    let value = registry
        .execute("overlap.tool", json!({}), Layer::Surface)
        .await
        .unwrap();
    assert_eq!(value["owner"], json!("a"));
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
        category: String::new(),
        keywords: Vec::new(),
        examples: Vec::new(),
        background: false,
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
        category: String::new(),
        keywords: Vec::new(),
        examples: Vec::new(),
        background: false,
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
fn confine_tool_path_allows_workspace_root_and_dot() {
    let dir = tempfile::TempDir::new().unwrap();
    let root =
        crate::pipeline::confine_tool_path(dir.path(), std::path::Path::new("."), false).unwrap();
    assert_eq!(root, dir.path().canonicalize().unwrap());
    let empty =
        crate::pipeline::confine_tool_path(dir.path(), std::path::Path::new(""), false).unwrap();
    assert_eq!(empty, root);
    let abs = crate::pipeline::confine_tool_path(dir.path(), dir.path(), false).unwrap();
    assert_eq!(abs, root);
}

#[test]
fn unknown_plugin_empty_side_effects_are_medium_sensitivity() {
    let spec = ene_plugin_ipc::ToolSpecWire {
        name: "evil.read".to_owned(),
        description: "d".to_owned(),
        parameters: json!({"type":"object"}),
        output: json!({"type":"object"}),
        side_effects: Vec::new(),
        broker_socket: None,
        category: String::new(),
        keywords: Vec::new(),
        examples: Vec::new(),
        background: false,
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
async fn start_background_rejects_sync_tools() {
    let registry = ToolRegistry::new();
    for def in crate::builtins::definitions_for(BuiltinKind::Utility) {
        registry.register(def);
    }
    let err = registry
        .start_background(
            "utility.hash",
            json!({"text": "x"}),
            "exec-1",
            Layer::Surface,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PipelineError::NotBackground(_)));
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
    let yen = registry
        .execute(
            "utility.calc",
            json!({"value": 1, "from": "USD", "to": "JPY"}),
            Layer::Surface,
        )
        .await
        .unwrap();
    assert_eq!(
        yen["source"],
        json!("ECB eurofxref daily (USD cross, rounded)")
    );
    assert!((yen["value"].as_f64().unwrap() - 150.0).abs() < 1e-9);
    let host = registry
        .execute("utility.system_info", json!({}), Layer::Surface)
        .await
        .unwrap();
    assert_eq!(host["os"], json!(std::env::consts::OS));
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
    let err = registry
        .execute(
            "web.fetch",
            json!({"url":"https://example.invalid/"}),
            Layer::Surface,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, PipelineError::Execute(message) if message.contains("host net broker")),
        "{err}"
    );
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
    let click = defs.iter().find(|def| def.name == "app.click");
    if let Some(click) = click {
        assert_eq!(click.side_effects, vec!["input".to_owned()]);
    }
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
    let listed_dot = registry
        .execute("fs.list", json!({"path": "."}), Layer::Surface)
        .await
        .unwrap();
    assert!(
        listed_dot["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| { row.get("name").and_then(Value::as_str) == Some("a.txt") })
    );
    let listed_abs = registry
        .execute(
            "fs.list",
            json!({"path": dir.path().to_string_lossy()}),
            Layer::Surface,
        )
        .await
        .unwrap();
    assert!(
        listed_abs["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| { row.get("name").and_then(Value::as_str) == Some("a.txt") })
    );
}

#[tokio::test]
async fn execute_in_workspace_ignores_registry_root() {
    let dir = tempfile::TempDir::new().unwrap();
    let global = dir.path().join("global");
    let job = dir.path().join("job");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::create_dir_all(&job).unwrap();
    std::fs::write(global.join("secret.txt"), "nope").unwrap();
    std::fs::write(job.join("only.txt"), "ok").unwrap();
    let registry = ToolRegistry::new();
    registry.set_workspace(&global);
    for def in crate::builtins::definitions_for(BuiltinKind::Fs) {
        registry.register(def);
    }
    let listed = registry
        .execute_in_workspace("fs.list", json!({}), Layer::Job, &job)
        .await
        .unwrap();
    let names: Vec<&str> = listed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row.get("name").and_then(Value::as_str))
        .collect();
    assert!(names.contains(&"only.txt"));
    assert!(!names.contains(&"secret.txt"));
}

#[test]
fn bundled_plugin_mains_own_logic_without_builtin_kind() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for name in ["fs", "exec", "web", "utility", "app"] {
        let main =
            std::fs::read_to_string(root.join(format!("plugins/tool/{name}/src/main.rs"))).unwrap();
        assert!(
            !main.contains("BuiltinKind"),
            "{name} must not launch via BuiltinKind"
        );
        assert!(
            main.contains("run_tool_plugin"),
            "{name} must call run_tool_plugin"
        );
        assert!(
            root.join(format!("plugins/tool/{name}/src/logic.rs"))
                .is_file(),
            "{name} logic.rs"
        );
    }
}

fn register_bundled_fs_utility(registry: &ToolRegistry) {
    for kind in [BuiltinKind::Fs, BuiltinKind::Utility] {
        for def in crate::builtins::definitions_for(kind) {
            registry.register(def);
        }
    }
}

#[test]
fn search_tools_ranks_keyword_match_first() {
    let registry = ToolRegistry::new();
    register_bundled_fs_utility(&registry);
    let hits = registry.search_tools("grep", 8);
    assert!(!hits.is_empty());
    assert_eq!(hits[0].tool.name, "fs.search");
    assert!(hits[0].score > 0);
}

#[test]
fn search_tools_works_without_embeddings() {
    let registry = ToolRegistry::new();
    register_bundled_fs_utility(&registry);
    let hits = registry.search_tools("hash", 4);
    assert!(hits.iter().any(|hit| hit.tool.name == "utility.hash"));
}

#[test]
fn search_tools_unregister_removes_plugin_tools() {
    let registry = ToolRegistry::new();
    register_bundled_fs_utility(&registry);
    registry.unregister_plugin("tool.fs");
    let hits = registry.search_tools("grep", 8);
    assert!(hits.iter().all(|hit| !hit.tool.name.starts_with("fs.")));
    assert!(
        registry
            .search_tools("hash", 4)
            .iter()
            .any(|hit| hit.tool.name == "utility.hash")
    );
}

#[test]
fn search_tools_tie_breaks_by_name() {
    let registry = ToolRegistry::new();
    registry.register(ToolDefinition {
        name: "aaa.match".to_owned(),
        description: "alpha tool".to_owned(),
        parameters: json!({"type":"object"}),
        output: json!({"type":"object"}),
        side_effects: Vec::new(),
        source: ToolSource::Harness {
            name: "test".to_owned(),
        },
        timeout_ms: None,
        sensitivity: ene_plane::Sensitivity::None,
        category: "test".to_owned(),
        keywords: vec!["needle".to_owned()],
        examples: Vec::new(),
        background: false,
    });
    registry.register(ToolDefinition {
        name: "zzz.match".to_owned(),
        description: "omega tool".to_owned(),
        parameters: json!({"type":"object"}),
        output: json!({"type":"object"}),
        side_effects: Vec::new(),
        source: ToolSource::Harness {
            name: "test".to_owned(),
        },
        timeout_ms: None,
        sensitivity: ene_plane::Sensitivity::None,
        category: "test".to_owned(),
        keywords: vec!["needle".to_owned()],
        examples: Vec::new(),
        background: false,
    });
    let hits = registry.search_tools("needle", 8);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].score, hits[1].score);
    assert_eq!(hits[0].tool.name, "aaa.match");
    assert_eq!(hits[1].tool.name, "zzz.match");
}

#[test]
fn search_tools_respects_limit() {
    let registry = ToolRegistry::new();
    register_bundled_fs_utility(&registry);
    let hits = registry.search_tools("", 3);
    assert_eq!(hits.len(), 3);
}

#[test]
fn search_tools_empty_query_is_name_sorted() {
    let registry = ToolRegistry::new();
    register_bundled_fs_utility(&registry);
    let hits = registry.search_tools("", 16);
    let names: Vec<&str> = hits.iter().map(|hit| hit.tool.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
    assert!(hits.iter().all(|hit| hit.score == 0));
}

#[test]
fn search_tools_truncates_huge_metadata() {
    let registry = ToolRegistry::new();
    registry.register(ToolDefinition {
        name: "huge.meta".to_owned(),
        description: "metadata cap".to_owned(),
        parameters: json!({"type":"object"}),
        output: json!({"type":"object"}),
        side_effects: Vec::new(),
        source: ToolSource::Harness {
            name: "test".to_owned(),
        },
        timeout_ms: None,
        sensitivity: ene_plane::Sensitivity::None,
        category: "x".repeat(256),
        keywords: vec!["x".repeat(256); 64],
        examples: vec!["y".repeat(512); 16],
        background: false,
    });
    let hits = registry.search_tools("huge", 1);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].tool.name, "huge.meta");
}

#[test]
fn search_tools_finds_mcp_tool_definition() {
    let registry = ToolRegistry::new();
    registry.register(ToolDefinition {
        name: "mcp:git.status".to_owned(),
        description: "Git working tree status".to_owned(),
        parameters: json!({"type":"object"}),
        output: json!({"type":"object"}),
        side_effects: Vec::new(),
        source: ToolSource::Mcp {
            server: "git".to_owned(),
        },
        timeout_ms: None,
        sensitivity: ene_plane::Sensitivity::None,
        category: "vcs".to_owned(),
        keywords: vec!["git".to_owned(), "status".to_owned()],
        examples: vec!["git status".to_owned()],
        background: false,
    });
    let hits = registry.search_tools("git status", 4);
    assert_eq!(hits[0].tool.name, "mcp:git.status");
}
