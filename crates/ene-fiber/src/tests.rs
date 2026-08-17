use crate::{FiberState, ProfileRow, Supervisor, discover_plugin_bin};
use ene_registry::ToolRegistry;
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
    drop(discover_plugin_bin("ene-harness-utility"));
}
