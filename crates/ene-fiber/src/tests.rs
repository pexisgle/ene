use crate::{
    CircuitBreakerConfig, FiberState, ProfileRow, Supervisor, discover_plugin_script,
    manifest_digest,
};
use ene_registry::{Layer, ToolRegistry};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

fn supervisor() -> (TempDir, Supervisor) {
    let dir = TempDir::new().unwrap();
    let registry = Arc::new(ToolRegistry::new());
    let sup = Supervisor::new(dir.path().to_path_buf(), registry);
    (dir, sup)
}

fn row(id: &str, plugin: &str, caps: &[&str]) -> ProfileRow {
    ProfileRow {
        row_id: id.to_owned(),
        plugin: plugin.to_owned(),
        requires: Vec::new(),
        capabilities: caps.iter().map(|s| (*s).to_owned()).collect(),
        sandbox_required: false,
    }
}

fn row_with_requires(id: &str, plugin: &str, requires: &[&str]) -> ProfileRow {
    ProfileRow {
        row_id: id.to_owned(),
        plugin: plugin.to_owned(),
        requires: requires.iter().map(|s| (*s).to_owned()).collect(),
        capabilities: Vec::new(),
        sandbox_required: false,
    }
}

#[tokio::test]
async fn unload_removes_tools_and_grants() {
    let (_dir, sup) = supervisor();
    let uid = sup
        .activate(&row("r-util", "tool.utility", &["fs.read"]))
        .unwrap();
    assert!(sup.surface_has_tool("utility.hash"));
    assert!(sup.broker_has_grant(uid, "fs.read"));
    sup.unload("r-util").await;
    assert!(!sup.surface_has_tool("utility.hash"));
    assert!(!sup.broker_has_grant(uid, "fs.read"));
    assert!(sup.fiber("r-util").is_none());
}

#[test]
fn loading_failure_does_not_leave_half_registration() {
    let (_dir, sup) = supervisor();
    let err = sup
        .activate(&row("r-bad", "tool.unknown", &[]))
        .unwrap_err();
    assert!(matches!(
        err,
        crate::supervisor::SupervisorError::UnknownPlugin(_)
    ));
    assert!(sup.registry().get("utility.hash").is_none());
}

#[tokio::test]
async fn disabling_one_row_leaves_the_other_active() {
    let (_dir, sup) = supervisor();
    let _uid_util = sup.activate(&row("r-util", "tool.utility", &[])).unwrap();
    let uid_web = sup.activate(&row("r-web", "tool.web", &[])).unwrap();
    sup.disable_row("r-util").await;
    let remaining = sup.fiber("r-web").unwrap();
    assert_eq!(remaining.uid, uid_web);
    assert_eq!(remaining.state, FiberState::Active);
    assert!(sup.surface_has_tool("web.fetch"));
    assert!(!sup.surface_has_tool("utility.hash"));
}

#[test]
fn undeclared_broker_op_is_denied() {
    let (dir, sup) = supervisor();
    let uid = sup.activate(&row("r-util", "tool.utility", &[])).unwrap();
    assert!(!sup.broker_has_grant(uid, "fs.write"));
    let path = dir.path().join("x.txt");
    std::fs::write(&path, "secret").unwrap();
    assert!(matches!(
        sup.broker_fs_read(uid, &path),
        Err(crate::BrokerError::Denied { .. })
    ));
}

#[tokio::test]
async fn reenable_allocates_a_new_uid() {
    let (_dir, sup) = supervisor();
    let first = sup.activate(&row("r-util", "tool.utility", &[])).unwrap();
    sup.disable_row("r-util").await;
    let second = sup.activate(&row("r-util", "tool.utility", &[])).unwrap();
    assert_ne!(first, second);
}

#[test]
fn discover_plugin_bin_is_optional() {
    drop(crate::discover_plugin_bin("ene-harness-utility"));
}

#[tokio::test]
async fn apply_profile_unloads_removed_rows_and_keeps_uid() {
    let (_dir, sup) = supervisor();
    let util_uid = sup.activate(&row("r-util", "tool.utility", &[])).unwrap();
    sup.activate(&row("r-web", "tool.web", &[])).unwrap();
    assert!(sup.surface_has_tool("utility.hash"));
    assert!(sup.surface_has_tool("web.fetch"));

    let report = sup
        .apply_profile(&[row("r-util", "tool.utility", &[])])
        .await;
    assert!(report.unloaded.contains(&"r-web".to_owned()));
    assert!(report.activated.is_empty());

    let util = sup.fiber("r-util").unwrap();
    assert_eq!(util.uid, util_uid);
    assert_eq!(util.state, FiberState::Active);
    assert!(sup.surface_has_tool("utility.hash"));
    assert!(!sup.surface_has_tool("web.fetch"));
    assert!(sup.fiber("r-web").is_none());
}

#[tokio::test]
async fn requires_unsatisfied_row_waits_without_error() {
    let (_dir, sup) = supervisor();
    let dependent = row_with_requires("r-web", "tool.web", &["tool.utility.hash"]);
    let report = sup.apply_profile(&[dependent]).await;
    assert!(report.waiting.contains(&"r-web".to_owned()));
    let fiber = sup.fiber("r-web").unwrap();
    assert_eq!(fiber.state, FiberState::Waiting);
    assert!(!sup.surface_has_tool("web.fetch"));
    assert_eq!(
        sup.missing_requires_for("r-web"),
        vec!["tool.utility.hash".to_owned()]
    );

    let report = sup
        .apply_profile(&[
            row("r-util", "tool.utility", &[]),
            row_with_requires("r-web", "tool.web", &["tool.utility.hash"]),
        ])
        .await;
    assert!(report.activated.contains(&"r-web".to_owned()));
    assert!(sup.surface_has_tool("web.fetch"));
}

#[tokio::test]
async fn circular_requires_are_reported_and_rows_stay_inactive() {
    let (_dir, sup) = supervisor();
    let rows = vec![
        row_with_requires("r-util", "tool.utility", &["tool.web.fetch"]),
        row_with_requires("r-web", "tool.web", &["tool.utility.hash"]),
    ];
    let report = sup.apply_profile(&rows).await;
    assert_eq!(report.cycle_rows, vec!["r-util", "r-web"]);
    assert!(sup.cycle_report().is_some());
    assert!(!sup.surface_has_tool("utility.hash"));
    assert!(!sup.surface_has_tool("web.fetch"));
    let util = sup.fiber("r-util").unwrap();
    assert_eq!(util.state, FiberState::Inactive);
    let web = sup.fiber("r-web").unwrap();
    assert_eq!(web.state, FiberState::Inactive);
}

#[tokio::test]
async fn circuit_breaker_opens_after_spawn_failures() {
    let dir = TempDir::new().unwrap();
    let sup = Supervisor::with_config(
        dir.path().to_path_buf(),
        Arc::new(ToolRegistry::new()),
        CircuitBreakerConfig { max_failures: 3 },
    );
    let row = row("r-dummy", "tool.dummy", &[]);
    let missing = dir.path().join("missing-plugin");
    for _ in 0..3 {
        drop(sup.activate_process(&row, &missing).await);
    }
    assert_eq!(sup.failure_count("r-dummy"), 3);
    assert!(sup.circuit_open("r-dummy"));
    assert!(!sup.surface_has_tool("dummy.ping"));
    let err = sup.activate_process(&row, &missing).await.unwrap_err();
    assert!(matches!(
        err,
        crate::supervisor::SupervisorError::CircuitOpen(_)
    ));
}

#[test]
fn manifest_digest_matches_python_plugin_contract() {
    let digest = manifest_digest("tool.dummy");
    assert!(digest.starts_with("sha256:"));
    assert_eq!(digest.len(), "sha256:".len() + 64);
}

fn dummy_plugin_path() -> PathBuf {
    discover_plugin_script("plugin.py").expect("dummy plugin script must exist")
}

#[tokio::test]
async fn python_dummy_registers_in_registry_and_executes() {
    let (_dir, sup) = supervisor();
    let path = dummy_plugin_path();
    let row = row("r-dummy", "tool.dummy", &[]);
    let uid = sup.activate_process(&row, &path).await.unwrap();
    assert!(!uid.to_string().is_empty());
    assert!(sup.surface_has_tool("dummy.ping"));
    let listed = sup
        .registry()
        .list()
        .into_iter()
        .map(|def| def.name)
        .collect::<Vec<_>>();
    assert!(listed.contains(&"dummy.ping".to_owned()));
    let value = sup
        .registry()
        .execute("dummy.ping", json!({"message": "hi"}), Layer::Surface)
        .await
        .unwrap();
    assert_eq!(value.get("pong").and_then(|v| v.as_str()), Some("hi"));
    sup.unload("r-dummy").await;
    assert!(!sup.surface_has_tool("dummy.ping"));
}

#[tokio::test]
async fn dummy_plugin_handshake_without_provider_subprotocol() {
    let path = dummy_plugin_path();
    let row = row("r-dummy", "tool.dummy", &[]);
    let (_dir, sup) = supervisor();
    sup.activate_process(&row, &path)
        .await
        .expect("core+tool handshake must succeed without provider");
    assert!(sup.surface_has_tool("dummy.ping"));
}
