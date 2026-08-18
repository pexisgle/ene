use crate::{
    Broker, BrokerError, CircuitBreakerConfig, Effect, FiberState, FiberUid, ProfileRow,
    SidecarRequest, Supervisor, confine_path, discover_plugin_script, manifest_digest,
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
        config: serde_json::Value::Null,
    }
}

fn row_with_requires(id: &str, plugin: &str, requires: &[&str]) -> ProfileRow {
    ProfileRow {
        row_id: id.to_owned(),
        plugin: plugin.to_owned(),
        requires: requires.iter().map(|s| (*s).to_owned()).collect(),
        capabilities: Vec::new(),
        sandbox_required: false,
        config: serde_json::Value::Null,
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
    assert!(sup.registry().get("web.fetch").is_some());
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
    assert!(sup.registry().get("web.fetch").is_some());
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
    assert!(sup.registry().get("web.fetch").is_none());
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
    assert!(sup.registry().get("web.fetch").is_some());
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
    let bad = dir.path().join("bad-plugin.py");
    std::fs::write(&bad, b"not a plugin").unwrap();
    for _ in 0..3 {
        drop(sup.activate_process(&row, &bad).await);
    }
    assert_eq!(sup.failure_count("r-dummy"), 3);
    assert!(sup.circuit_open("r-dummy"));
    assert!(!sup.surface_has_tool("dummy.ping"));
    let err = sup.activate_process(&row, &bad).await.unwrap_err();
    assert!(matches!(
        err,
        crate::supervisor::SupervisorError::CircuitOpen(_)
    ));
}

#[test]
fn manifest_digest_matches_python_plugin_contract() {
    let path = dummy_plugin_path();
    let digest = manifest_digest(&path).unwrap();
    assert!(digest.starts_with("blake3:"));
    assert_eq!(digest.len(), "blake3:".len() + 64);
}

fn dummy_plugin_path() -> PathBuf {
    discover_plugin_script("plugin.py").expect("dummy plugin script must exist")
}

#[tokio::test]
async fn python_dummy_registers_in_registry_and_executes() {
    if python3_bin().is_none() {
        return;
    }
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
    if python3_bin().is_none() {
        return;
    }
    let path = dummy_plugin_path();
    let row = row("r-dummy", "tool.dummy", &[]);
    let (_dir, sup) = supervisor();
    sup.activate_process(&row, &path)
        .await
        .expect("core+tool handshake must succeed without provider");
    assert!(sup.surface_has_tool("dummy.ping"));
}

#[tokio::test]
async fn dropping_supervisor_kills_spawned_plugin() {
    if python3_bin().is_none() {
        return;
    }
    let dir = TempDir::new().unwrap();
    let sup = Supervisor::new(dir.path().to_path_buf(), Arc::new(ToolRegistry::new()));
    let path = dummy_plugin_path();
    let row = row("r-dummy", "tool.dummy", &[]);
    sup.activate_process(&row, &path).await.unwrap();
    let pid = sup
        .fiber("r-dummy")
        .unwrap()
        .dispose
        .iter()
        .find_map(|effect| match effect {
            Effect::SpawnProcess { pid } => Some(*pid),
            _ => None,
        })
        .expect("spawn pid");
    assert!(
        std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "plugin child {pid} should be running"
    );
    drop(sup);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "plugin child {pid} still alive after Supervisor drop"
    );
}

#[test]
fn broker_write_via_parent_escape_is_path_escape() {
    let dir = TempDir::new().unwrap();
    let mut broker = Broker::new(dir.path().to_path_buf());
    let uid = FiberUid::new();
    broker.grant(uid, "fs.write");
    assert!(matches!(
        broker.fs_write(uid, PathBuf::from("../outside.txt").as_path(), "nope"),
        Err(BrokerError::PathEscape(_))
    ));
}

#[test]
fn broker_write_inside_workspace_succeeds() {
    let dir = TempDir::new().unwrap();
    let mut broker = Broker::new(dir.path().to_path_buf());
    let uid = FiberUid::new();
    broker.grant(uid, "fs.write");
    broker
        .fs_write(uid, PathBuf::from("inside.txt").as_path(), "ok")
        .unwrap();
    let body = std::fs::read_to_string(dir.path().join("inside.txt")).unwrap();
    assert_eq!(body, "ok");
}

#[test]
fn confine_path_rejects_workspace_parent_escape_for_new_files() {
    let dir = TempDir::new().unwrap();
    assert!(matches!(
        confine_path(dir.path(), PathBuf::from("../escape.txt").as_path(), true),
        Err(BrokerError::PathEscape(_))
    ));
}

#[test]
fn sidecar_binary_resolves_config_then_cas_then_bundled_and_rejects_urls() {
    let dir = TempDir::new().unwrap();
    let bundled_dir = dir.path().join("bundled");
    std::fs::create_dir(&bundled_dir).unwrap();
    let config = dir.path().join("from-config");
    let cas = dir.path().join("from-cas");
    let bundled = bundled_dir.join("engine");
    std::fs::write(&config, b"c").unwrap();
    std::fs::write(&cas, b"a").unwrap();
    std::fs::write(&bundled, b"b").unwrap();
    let broker = Broker::with_bundled_dir(dir.path().to_path_buf(), bundled_dir);

    let all = SidecarRequest {
        config_path: Some(config.clone()),
        cas_path: Some(cas.clone()),
        bundled_name: "engine".into(),
        args: Vec::new(),
    };
    assert_eq!(broker.resolve_sidecar_binary(&all).unwrap(), config);

    let cas_only = SidecarRequest {
        config_path: None,
        cas_path: Some(cas.clone()),
        bundled_name: "engine".into(),
        args: Vec::new(),
    };
    assert_eq!(broker.resolve_sidecar_binary(&cas_only).unwrap(), cas);

    let bundled_only = SidecarRequest {
        config_path: None,
        cas_path: None,
        bundled_name: "engine".into(),
        args: Vec::new(),
    };
    assert_eq!(
        broker.resolve_sidecar_binary(&bundled_only).unwrap(),
        bundled.canonicalize().unwrap()
    );

    let remote = SidecarRequest {
        config_path: Some(PathBuf::from("https://example.invalid/llama-server")),
        cas_path: None,
        bundled_name: String::new(),
        args: Vec::new(),
    };
    assert!(matches!(
        broker.resolve_sidecar_binary(&remote),
        Err(BrokerError::RemoteBinaryForbidden)
    ));

    let file_url = SidecarRequest {
        config_path: Some(PathBuf::from("file:///etc/passwd")),
        cas_path: None,
        bundled_name: String::new(),
        args: Vec::new(),
    };
    assert!(matches!(
        broker.resolve_sidecar_binary(&file_url),
        Err(BrokerError::RemoteBinaryForbidden)
    ));

    let evil = SidecarRequest {
        config_path: None,
        cas_path: None,
        bundled_name: "../evil".into(),
        args: Vec::new(),
    };
    assert!(matches!(
        broker.resolve_sidecar_binary(&evil),
        Err(BrokerError::SidecarBinaryNotFound)
    ));

    let missing = SidecarRequest {
        config_path: None,
        cas_path: None,
        bundled_name: String::new(),
        args: Vec::new(),
    };
    assert!(matches!(
        broker.resolve_sidecar_binary(&missing),
        Err(BrokerError::SidecarBinaryNotFound)
    ));
}

const LOOPBACK_PY: &str = r"
import socket, sys, time
port = int(sys.argv[1])
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
sock.bind(('127.0.0.1', port))
sock.listen(1)
time.sleep(3600)
";

fn python3_bin() -> Option<PathBuf> {
    let output = std::process::Command::new("sh")
        .args(["-c", "command -v python3"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

#[test]
fn sidecar_spawn_health_and_kill_on_loopback() {
    let Some(python) = python3_bin() else {
        return;
    };
    let dir = TempDir::new().unwrap();
    let mut broker = Broker::new(dir.path().to_path_buf());
    let uid = FiberUid::new();
    let request = SidecarRequest {
        config_path: Some(python),
        cas_path: None,
        bundled_name: String::new(),
        args: vec!["-c".into(), LOOPBACK_PY.into(), "{port}".into()],
    };
    assert!(matches!(
        broker.spawn_sidecar(uid, &request),
        Err(BrokerError::Denied { .. })
    ));
    broker.grant(uid, "proc.spawn_sidecar");
    let id = broker.spawn_sidecar(uid, &request).unwrap();
    let health = broker.sidecar_health(uid, id).unwrap();
    assert!(health.alive);
    assert_ne!(health.port, 0);
    let other = FiberUid::new();
    assert!(matches!(
        broker.sidecar_health(other, id),
        Err(BrokerError::SidecarNotOwned { .. })
    ));
    broker.kill_sidecar(uid, id).unwrap();
    assert!(matches!(
        broker.sidecar_health(uid, id),
        Err(BrokerError::UnknownSidecar(_))
    ));
}

#[test]
fn exe_plugin_candidates_include_plugins_dir() {
    let dir = PathBuf::from("/opt/ene");
    let found = crate::spawn::exe_plugin_candidates(&dir, "ene-harness-fs");
    assert_eq!(
        found,
        vec![
            dir.join("ene-harness-fs"),
            dir.join("plugins").join("ene-harness-fs"),
        ]
    );
}

#[test]
fn discover_plugin_executable_in_searches_home() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("ene-harness-utility");
    std::fs::write(&binary, b"#!/bin/true\n").unwrap();
    let found = crate::spawn::discover_plugin_executable_in("tool.utility", Some(dir.path()));
    assert_eq!(found.as_deref(), Some(binary.as_path()));
}
