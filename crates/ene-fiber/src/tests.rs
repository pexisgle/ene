use crate::{
    Broker, BrokerError, CircuitBreakerConfig, Effect, FiberState, FiberUid, ProfileRow, SidecarId,
    SidecarRequest, Supervisor, SupervisorError, confine_path, discover_plugin_script,
    manifest_digest,
};
use ene_registry::{Layer, ToolRegistry};
use parking_lot::Mutex;
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use tempfile::TempDir;

static POSTS: LazyLock<Mutex<Vec<serde_json::Value>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn supervisor() -> (TempDir, Supervisor) {
    let dir = TempDir::new().unwrap();
    let registry = Arc::new(ToolRegistry::new());
    let sup = Supervisor::new(dir.path().to_path_buf(), registry);
    sup.set_prefer_in_process_builtins(true);
    (dir, sup)
}

fn row(id: &str, plugin: &str, caps: &[&str]) -> ProfileRow {
    ProfileRow {
        row_id: id.to_owned(),
        plugin: plugin.to_owned(),
        requires: Vec::new(),
        capabilities: caps.iter().map(|s| (*s).to_owned()).collect(),
        seams: Vec::new(),
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
        seams: Vec::new(),
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

#[tokio::test]
async fn unloading_one_fiber_keeps_another_fibers_tool() {
    let (_dir, sup) = supervisor();
    sup.activate(&row("r-a", "tool.utility", &[])).unwrap();
    sup.activate(&row("r-b", "tool.utility", &[])).unwrap();
    assert!(sup.surface_has_tool("utility.hash"));
    sup.unload("r-a").await;
    assert!(sup.surface_has_tool("utility.hash"));
    assert!(sup.fiber("r-b").is_some());
    sup.unload("r-b").await;
    assert!(!sup.surface_has_tool("utility.hash"));
}

#[tokio::test]
async fn unloading_the_later_fiber_restores_the_earlier_tool() {
    let (_dir, sup) = supervisor();
    sup.activate(&row("r-a", "tool.utility", &[])).unwrap();
    sup.activate(&row("r-b", "tool.utility", &[])).unwrap();
    sup.unload("r-b").await;
    assert!(sup.surface_has_tool("utility.hash"));
    assert!(sup.fiber("r-a").is_some());
    sup.unload("r-a").await;
    assert!(!sup.surface_has_tool("utility.hash"));
}

#[tokio::test]
async fn dispose_inverts_every_effect_kind_lifo() {
    use ene_plugin_ipc::BuiltinKind;
    use ene_registry::definitions_for;

    let (_dir, sup) = supervisor();
    let hooks = ene_kernel::LoopHooks::new();
    sup.set_loop_hooks(hooks);
    let uid = sup
        .activate(&row("r-util", "tool.utility", &["fs.read"]))
        .unwrap();
    sup.push_effect(
        "r-util",
        Effect::BindSeam {
            name: "llm".to_owned(),
        },
    );
    sup.push_effect("r-util", Effect::SpawnProcess { pid: 1 });
    sup.listen_pre_step("r-util", |event, _next| event).unwrap();
    let tool_names: Vec<String> = definitions_for(BuiltinKind::Utility)
        .into_iter()
        .map(|def| def.name)
        .rev()
        .collect();
    sup.unload("r-util").await;
    let inverted = sup.last_dispose();
    let mut expected = vec![
        Effect::ListenWaterfall {
            point: "agent/pre-step".to_owned(),
        },
        Effect::SpawnProcess { pid: 1 },
        Effect::BindSeam {
            name: "llm".to_owned(),
        },
        Effect::BrokerGrant {
            op: "fs.read".to_owned(),
        },
    ];
    expected.extend(tool_names.into_iter().map(|name| Effect::RegisterTool {
        name,
        owner: uid.to_string(),
    }));
    assert_eq!(inverted, expected);
    assert!(!sup.surface_has_tool("utility.hash"));
}

#[test]
fn rollback_loading_uses_the_same_dispose_path() {
    let (_dir, sup) = supervisor();
    sup.activate(&row("r-util", "tool.utility", &["fs.read"]))
        .unwrap();
    assert!(sup.surface_has_tool("utility.hash"));
    sup.rollback_active("r-util");
    assert!(!sup.surface_has_tool("utility.hash"));
    assert!(sup.fiber("r-util").is_none());
    assert!(
        sup.last_dispose()
            .iter()
            .any(|effect| matches!(effect, Effect::BrokerGrant { op } if op == "fs.read"))
    );
    assert!(sup.last_dispose().iter().any(
        |effect| matches!(effect, Effect::RegisterTool { name, .. } if name == "utility.hash")
    ));
}

#[tokio::test]
async fn fiber_pre_step_listen_unregisters_on_unload() {
    let (_dir, sup) = supervisor();
    let hooks = ene_kernel::LoopHooks::new();
    sup.set_loop_hooks(hooks.clone());
    sup.activate(&row("r-util", "tool.utility", &[])).unwrap();
    sup.listen_pre_step("r-util", |mut event, _next| {
        event.proceed = false;
        event.note = "fiber intercept".into();
        event
    })
    .unwrap();
    let blocked = hooks.pre_step.run(ene_kernel::HookEvent::default());
    assert!(!blocked.proceed);
    assert_eq!(blocked.note, "fiber intercept");
    sup.unload("r-util").await;
    let after = hooks.pre_step.run(ene_kernel::HookEvent::default());
    assert!(after.proceed);
    assert!(after.note.is_empty());
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
    drop(crate::discover_plugin_bin("ene-tool-utility"));
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
    let row = row("r-dummy-circuit", "tool.dummy", &[]);
    let bad = dir.path().join("bad-plugin.py");
    std::fs::write(&bad, b"not a plugin").unwrap();
    for _ in 0..3 {
        drop(sup.activate_process(&row, &bad).await);
    }
    assert_eq!(sup.failure_count("r-dummy-circuit"), 3);
    assert!(sup.circuit_open("r-dummy-circuit"));
    assert!(!sup.surface_has_tool("dummy.ping"));
    let err = sup.activate_process(&row, &bad).await.unwrap_err();
    assert!(matches!(
        err,
        crate::supervisor::SupervisorError::CircuitOpen(_)
    ));
    let failed = sup.fiber("r-dummy-circuit").expect("failed fiber stays");
    assert_eq!(failed.state, FiberState::Failed);
    assert!(failed.dispose.is_empty());
}

#[tokio::test]
async fn apply_profile_keeps_failed_spawn_visible() {
    let (_dir, sup) = supervisor();
    let report = sup
        .apply_profile(&[row("r-missing", "provider.no_such_vendor", &[])])
        .await;
    assert!(report.waiting.contains(&"r-missing".to_owned()));
    let fiber = sup.fiber("r-missing").expect("waiting fiber");
    assert_eq!(fiber.state, FiberState::Waiting);
    assert!(fiber.wait_reason.is_some());
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
    let row = row("r-dummy-exec", "tool.dummy", &[]);
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
    sup.unload("r-dummy-exec").await;
    assert!(!sup.surface_has_tool("dummy.ping"));
}

#[tokio::test]
async fn dummy_plugin_handshake_without_provider_subprotocol() {
    if python3_bin().is_none() {
        return;
    }
    let path = dummy_plugin_path();
    let row = row("r-dummy-handshake", "tool.dummy", &[]);
    let (_dir, sup) = supervisor();
    sup.activate_process(&row, &path)
        .await
        .expect("core+tool handshake must succeed without provider");
    assert!(sup.surface_has_tool("dummy.ping"));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn dropping_supervisor_kills_spawned_plugin() {
    if python3_bin().is_none() {
        return;
    }
    let dir = TempDir::new().unwrap();
    let sup = Supervisor::new(dir.path().to_path_buf(), Arc::new(ToolRegistry::new()));
    let path = dummy_plugin_path();
    let row = row("r-dummy-drop", "tool.dummy", &[]);
    sup.activate_process(&row, &path).await.unwrap();
    let pid = sup
        .fiber("r-dummy-drop")
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
import socket, sys
port = int(sys.argv[1])
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
sock.bind(('127.0.0.1', port))
# Accept health probes: listen-only + backlog=1 fills after the first
# connect on Windows, so a second tcp_open reports not alive.
sock.listen(16)
while True:
    conn, _ = sock.accept()
    conn.close()
";

fn python3_bin() -> Option<PathBuf> {
    for candidate in ["python3", "python"] {
        let Ok(output) = std::process::Command::new(candidate)
            .args(["-c", "import sys; print(sys.executable)"])
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    None
}

/// The Windows runner can stall interpreter startup long enough to exhaust
/// the production two-second health budget; retry once before treating the
/// test as failed.
fn spawn_loopback_sidecar_with_retry(
    broker: &mut Broker,
    uid: FiberUid,
    request: &SidecarRequest,
) -> Result<SidecarId, BrokerError> {
    match broker.spawn_sidecar(uid, request) {
        Err(BrokerError::SidecarUnhealthy)
            if cfg!(target_os = "windows")
                && std::env::var_os("ENE_FIBER_SIDECAR_TEST_NO_RETRY").is_none() =>
        {
            tracing::warn!("sidecar health check timed out; retrying Windows test spawn");
            broker.spawn_sidecar(uid, request)
        }
        result => result,
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
    let id = spawn_loopback_sidecar_with_retry(&mut broker, uid, &request).unwrap();
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
    let found = crate::spawn::exe_plugin_candidates(&dir, "ene-tool-fs");
    #[cfg(windows)]
    assert_eq!(
        found,
        vec![
            dir.join("ene-tool-fs"),
            dir.join("ene-tool-fs.exe"),
            dir.join("plugins").join("ene-tool-fs"),
            dir.join("plugins").join("ene-tool-fs.exe"),
        ]
    );
    #[cfg(not(windows))]
    assert_eq!(
        found,
        vec![
            dir.join("ene-tool-fs"),
            dir.join("plugins").join("ene-tool-fs"),
        ]
    );
}

#[test]
fn discover_plugin_executable_in_searches_home() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("ene-tool-utility");
    std::fs::write(&binary, b"#!/bin/true\n").unwrap();
    let found = crate::spawn::discover_plugin_executable_in("tool.utility", Some(dir.path()));
    assert_eq!(found.as_deref(), Some(binary.as_path()));
}

#[tokio::test]
async fn list_models_unknown_kind_is_unknown_plugin() {
    let (_dir, sup) = supervisor();
    let err = sup
        .list_models(
            "not-a-plugin",
            ene_plugin_ipc::ListModelsRequest {
                seam: "seam.llm".to_owned(),
                base_url: String::new(),
                auth: ene_plugin_ipc::ProviderAuth::default(),
            },
        )
        .await
        .expect_err("unknown");
    assert!(matches!(err, SupervisorError::UnknownPlugin(_)));
}

#[tokio::test]
async fn list_models_probe_without_binary_is_spawn_error() {
    let (_dir, sup) = supervisor();
    let err = sup
        .list_models(
            "provider.no_such_vendor",
            ene_plugin_ipc::ListModelsRequest {
                seam: "seam.llm".to_owned(),
                base_url: String::new(),
                auth: ene_plugin_ipc::ProviderAuth::default(),
            },
        )
        .await
        .expect_err("missing");
    assert!(
        matches!(&err, SupervisorError::Spawn(msg) if msg.contains("missing")),
        "expected spawn error containing missing, got {err}"
    );
}

#[tokio::test]
async fn list_models_probe_does_not_peer_close() {
    let (_dir, sup) = supervisor();
    if sup.discover("provider.openai_compat").is_none() {
        return;
    }
    let listed = sup
        .list_models(
            "provider.openai_compat",
            ene_plugin_ipc::ListModelsRequest {
                seam: "seam.llm".to_owned(),
                base_url: "http://127.0.0.1:1".to_owned(),
                auth: ene_plugin_ipc::ProviderAuth::default(),
            },
        )
        .await;
    if let Err(err) = listed {
        assert!(
            !err.to_string().contains("peer closed"),
            "probe IPC must stay up: {err}"
        );
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "matches the net fetch stub signature"
)]
fn stub_net_fetch_ok(_: &str) -> Result<serde_json::Value, BrokerError> {
    Ok(json!({
        "status": 200,
        "content_type": "text/plain",
        "text": "ok",
    }))
}

#[test]
fn net_fetch_without_grant_is_denied() {
    let dir = TempDir::new().unwrap();
    let broker = Broker::new(dir.path().to_path_buf());
    let uid = FiberUid::new();
    assert!(matches!(
        broker.net_fetch(uid, "https://example.invalid/v1"),
        Err(BrokerError::Denied { .. })
    ));
}

#[test]
fn net_fetch_blocks_loopback_after_grant() {
    let dir = TempDir::new().unwrap();
    let mut broker = Broker::new(dir.path().to_path_buf());
    let uid = FiberUid::new();
    broker.grant(uid, "net.fetch");
    assert!(matches!(
        broker.net_fetch(uid, "http://127.0.0.1/secret"),
        Err(BrokerError::Ssrf(_))
    ));
}

#[test]
fn net_fetch_runs_after_grant() {
    let dir = TempDir::new().unwrap();
    let mut broker = Broker::new(dir.path().to_path_buf());
    let uid = FiberUid::new();
    broker.grant(uid, "net.fetch");
    let value = crate::net::with_fetch_stub(stub_net_fetch_ok, || {
        broker.net_fetch(uid, "https://example.invalid/v1")
    })
    .unwrap();
    assert_eq!(value["status"], 200);
    assert_eq!(value["text"], "ok");
}

#[tokio::test]
async fn probe_search_backends_reports_injected_credentials() {
    let (_dir, sup) = supervisor();
    let mut r = row("r-web", "tool.web", &["net.fetch"]);
    r.config = json!({"tavily_api_key": "tvly-live"});
    sup.activate(&r).unwrap();
    sup.grant_for_tests(sup.fiber("r-web").unwrap().uid, "net.fetch");
    ene_registry::with_post_json(
        move |_url, _body, _bearer| {
            Ok(json!({"status": 200, "content_type": "application/json", "results": []}))
        },
        || async {
            let v = sup
                .registry()
                .execute("web.search_backends", json!({}), Layer::Surface)
                .await
                .unwrap();
            assert_eq!(v["backends"][2]["id"], "tavily");
            assert_eq!(v["backends"][2]["available"], true);
            Ok::<(), ene_registry::PipelineError>(())
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn host_web_invoker_injects_config_credentials_into_builtin_search() {
    let (_dir, sup) = supervisor();
    let mut r = row("r-web", "tool.web", &["net.fetch"]);
    r.config = json!({"tavily_api_key": "tvly-live"});
    sup.activate(&r).unwrap();
    sup.grant_for_tests(sup.fiber("r-web").unwrap().uid, "net.fetch");

    fn capture_post(
        url: &str,
        body: &serde_json::Value,
        _bearer: Option<&str>,
    ) -> serde_json::Value {
        POSTS
            .lock()
            .push(json!({ "url": url, "body_api_key": body["api_key"] }));
        json!({
            "status": 200,
            "content_type": "application/json",
            "results": [
                {"title": "Tokyo", "url": "https://example.invalid/tokyo", "content": "Capital"}
            ]
        })
    }

    let result = crate::net::with_post_stub(capture_post, || async {
        sup.registry()
            .execute(
                "web.search",
                json!({"query": "tokyo", "backend": "tavily"}),
                Layer::Surface,
            )
            .await
    })
    .await
    .unwrap();
    assert_eq!(result["backend"], "tavily");
    let captured = POSTS.lock();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0]["url"], "https://api.tavily.com/search");
    assert_eq!(captured[0]["body_api_key"], "tvly-live");
}

#[test]
fn plugin_config_values_omit_secrets() {
    let (_dir, sup) = supervisor();
    let mut r = row("r-util", "tool.utility", &[]);
    r.config = json!({"model": "keep", "api_key": "sk-live"});
    let _ = sup.activate(&r).unwrap();
    let schema = json!({
        "type": "object",
        "properties": {
            "model": { "type": "string" },
            "api_key": { "type": "string", "x-ene-secret": true }
        }
    });
    let values = sup.plugin_config_values("r-util", &schema);
    assert_eq!(values["model"], "keep");
    assert!(values.get("api_key").is_none());
    assert!(!format!("{values:?}").contains("sk-live"));
}

#[test]
fn commit_applied_config_keeps_previous_on_failure() {
    let rows = parking_lot::Mutex::new(std::collections::HashMap::from([(
        "r-util".to_owned(),
        row("r-util", "tool.utility", &[]),
    )]));
    rows.lock().get_mut("r-util").unwrap().config = json!({"model": "keep"});
    crate::supervisor::commit_applied_config(
        &rows,
        "r-util",
        json!({"model": "new"}),
        false,
        json!({"model": "keep"}),
    );
    assert_eq!(rows.lock().get("r-util").unwrap().config["model"], "keep");
    crate::supervisor::commit_applied_config(
        &rows,
        "r-util",
        json!({"model": "new"}),
        true,
        json!({"model": "keep"}),
    );
    assert_eq!(rows.lock().get("r-util").unwrap().config["model"], "new");
}

#[tokio::test]
async fn apply_without_session_keeps_previous_config() {
    let (_dir, sup) = supervisor();
    let mut r = row("r-util", "tool.utility", &[]);
    r.config = json!({"model": "keep"});
    let _ = sup.activate(&r).unwrap();
    assert!(
        sup.plugin_config_apply("r-util", json!({"model": "new"}))
            .await
            .is_err()
    );
    assert_eq!(sup.profile_row("r-util").unwrap().config["model"], "keep");
    let schema = sup.plugin_config_schema("r-util").await.unwrap();
    assert!(!schema.has_config);
}

#[tokio::test]
async fn web_fetch_without_net_grant_is_denied() {
    let (_dir, sup) = supervisor();
    sup.activate(&row("r-web", "tool.web", &[])).unwrap();
    let err = sup
        .registry()
        .execute(
            "web.fetch",
            json!({"url": "https://example.invalid/"}),
            Layer::Surface,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("denied net.fetch"), "{err}");
}

#[tokio::test]
async fn web_fetch_with_grant_blocks_loopback() {
    let (_dir, sup) = supervisor();
    sup.activate(&row("r-web", "tool.web", &["net.fetch"]))
        .unwrap();
    let err = sup
        .registry()
        .execute(
            "web.fetch",
            json!({"url": "http://127.0.0.1/secret"}),
            Layer::Surface,
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("private") || err.to_string().contains("ssrf"),
        "{err}"
    );
}

#[test]
fn file_broker_glob_and_delete_stay_in_workspace() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a.rs"), "a").unwrap();
    std::fs::write(dir.path().join("gone.txt"), "x").unwrap();
    let mut broker = Broker::new(dir.path().to_path_buf());
    let uid = FiberUid::new();
    assert!(matches!(
        broker.fs_glob(uid, "**/*.rs"),
        Err(BrokerError::Denied { .. })
    ));
    broker.grant(uid, "fs.glob");
    broker.grant(uid, "fs.delete");
    broker.grant(uid, "fs.list");
    let globbed = broker.fs_glob(uid, "**/*.rs").unwrap();
    assert_eq!(globbed, vec!["src/a.rs".to_owned()]);
    assert!(matches!(
        broker.fs_glob(uid, "../outside"),
        Err(BrokerError::PathEscape(_))
    ));
    broker
        .fs_delete(uid, PathBuf::from("gone.txt").as_path())
        .unwrap();
    assert!(!dir.path().join("gone.txt").exists());
    std::fs::create_dir(dir.path().join("nested")).unwrap();
    std::fs::write(dir.path().join("nested/b.txt"), "b").unwrap();
    assert!(matches!(
        broker.fs_delete(uid, PathBuf::from("nested").as_path()),
        Err(BrokerError::NotEmpty)
    ));
}
