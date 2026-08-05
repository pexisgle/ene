//! Host-side `ResourceClass` admission contract tests over the real IPC path.
//!
//! Two mock "plugins" (each a separate IPC server) declare the same
//! `ResourceClass::Gpu`; the host must share one admission budget between
//! them, keep different devices independent, and release the budget when a
//! serving process dies — either via transport failure (server task aborted)
//! or a real SIGKILL of a child process (self-exec of this test binary).
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests use unwrap/expect/panic for assertions"
)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use tokio::sync::watch;

use ene_ai::message::{LlmMessage, UserMessagePart};
use ene_ai::traits::LlmProviderFactory;
use ene_config::EneConfig;
use ene_plugin_host::IpcPluginConnection;
use ene_plugin_host::admission::ResourceClassAdmission;
use ene_plugin_host::config::ResourceClassBudget;
use ene_plugin_host::factory::IpcLlmProviderFactory;
use ene_plugin_proto::{
    ConcurrencyHint, IpcListener, LlmProviderSpec, PLUGIN_IPC_PROTOCOL_VERSION, PluginCapabilities,
    PluginIpcRequest, PluginIpcResponse, ResourceClass, SandboxConfigData, VersionRange,
    WireFormat, cleanup_path, read_plugin_request, write_plugin_response,
};
use tokio_stream::StreamExt;

/// Counter for generating unique socket paths across parallel tests.
static SOCKET_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Handshake timeout used by the integration tests.
const TEST_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Concurrency bound passed to [`IpcPluginConnection::connect`].
const TEST_MAX_CONCURRENT: usize = 8;

/// Provider kind both mock plugins declare.
const MOCK_KIND: &str = "admission-mock";

fn test_socket_path(name: &str) -> PathBuf {
    let id = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!(
        "/tmp/ene-admission-test-{}-{id}-{name}.sock",
        std::process::id()
    ))
}

fn mock_spec(resource_class: ResourceClass) -> LlmProviderSpec {
    LlmProviderSpec {
        kind: MOCK_KIND.into(),
        supported_models: Vec::new(),
        supports_streaming: true,
        supports_vision: false,
        concurrency: ConcurrencyHint::default(),
        context_window: None,
        resource_class,
    }
}

/// Serves handshakes with a mock plugin declaring `resource_class`, switching
/// to the negotiated wire format after the handshake, then swallowing every
/// non-ping request so a stream stays genuinely in flight until the server
/// (or its connection) shuts down. Dropping the last `watch::Sender` for
/// `shutdown` closes all accepted connections — the in-process stand-in for
/// the serving process dying.
async fn run_admission_server(
    socket_path: PathBuf,
    resource_class: ResourceClass,
    mut shutdown: watch::Receiver<()>,
) {
    cleanup_path(&socket_path);
    let Ok(mut listener) = IpcListener::bind(&socket_path) else {
        return;
    };
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            accepted = listener.accept() => {
                let Ok(mut stream) = accepted else { break; };
                let mut shutdown = shutdown.clone();
                let capabilities = PluginCapabilities {
                    llm_providers: vec![mock_spec(resource_class)],
                    ..PluginCapabilities::default()
                };
                tokio::spawn(async move {
                    // The handshake frame is always JSON; the negotiated
                    // version decides the framing of every later frame.
                    let mut format = WireFormat::Json;
                    loop {
                        tokio::select! {
                            _ = shutdown.changed() => break,
                            frame = read_plugin_request(&mut stream, format) => {
                                let Ok(Some(req)) = frame else { break; };
                                match req {
                                    PluginIpcRequest::Handshake { version: host_range, .. } => {
                                        let negotiated = VersionRange {
                                            min: PLUGIN_IPC_PROTOCOL_VERSION,
                                            max: PLUGIN_IPC_PROTOCOL_VERSION,
                                        }
                                        .negotiate(&host_range)
                                        .unwrap_or(PLUGIN_IPC_PROTOCOL_VERSION);
                                        format = WireFormat::for_version(negotiated);
                                        if write_plugin_response(
                                            &mut stream,
                                            &PluginIpcResponse::HandshakeAck {
                                                version: negotiated,
                                                capabilities: capabilities.clone(),
                                            },
                                            WireFormat::Json,
                                        )
                                        .await
                                        .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    PluginIpcRequest::Ping { request_id } => {
                                        if write_plugin_response(
                                            &mut stream,
                                            &PluginIpcResponse::Pong { request_id },
                                            format,
                                        )
                                        .await
                                        .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    // Stream requests are deliberately never
                                    // answered, keeping the stream in flight.
                                    _ => {}
                                }
                            }
                        }
                    }
                });
            }
        }
    }
}

/// Connects a host connection to an in-process mock server.
async fn connect_mock(
    name: &str,
    resource_class: ResourceClass,
) -> (
    PathBuf,
    tokio::task::JoinHandle<()>,
    std::sync::Arc<IpcPluginConnection>,
) {
    let socket_path = test_socket_path(name);
    let (shutdown_tx, shutdown_rx) = watch::channel(());
    let server_path = socket_path.clone();
    let server = tokio::spawn(async move {
        // Keep the sender alive for the task's lifetime: when the task is
        // aborted (or exits), the sender drops and every accepted connection
        // observes shutdown and closes.
        let _sender = shutdown_tx;
        run_admission_server(server_path, resource_class, shutdown_rx).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let conn = IpcPluginConnection::connect(
        &socket_path,
        SandboxConfigData::default(),
        None,
        None,
        TEST_HANDSHAKE_TIMEOUT,
        TEST_MAX_CONCURRENT,
    )
    .await
    .expect("handshake with mock server");
    (socket_path, server, std::sync::Arc::new(conn))
}

fn mock_factory(
    conn: std::sync::Arc<IpcPluginConnection>,
    resource_class: ResourceClass,
    admission: &ResourceClassAdmission,
) -> IpcLlmProviderFactory {
    IpcLlmProviderFactory::new(
        MOCK_KIND.into(),
        conn,
        "admission-mock".into(),
        true,
        None,
        ConcurrencyHint::default(),
        resource_class,
        admission,
    )
}

/// Starts a chat stream on `factory`; the mock server never answers, so the
/// stream stays in flight and holds its class permit until dropped or the
/// server dies.
async fn start_hold_stream(
    factory: &IpcLlmProviderFactory,
    task: &ene_ai::TaskRef,
) -> Result<
    std::pin::Pin<
        Box<
            dyn tokio_stream::Stream<
                    Item = Result<
                        ene_ai::message::LlmResponseChunk,
                        ene_ai::error::LlmProviderError,
                    >,
                > + Send,
        >,
    >,
    ene_ai::error::LlmProviderError,
> {
    let config = EneConfig::default();
    let provider = factory.create_provider(&config, task)?;
    provider
        .create_chat_stream(
            &[LlmMessage::User {
                parts: vec![UserMessagePart::Text {
                    text: "hello".into(),
                }],
            }],
            &[],
        )
        .await
}

/// Waits until `stream` ends, with a bound so a leaked permit fails the test
/// instead of hanging it.
async fn drain_until_end<S, E>(stream: S)
where
    S: tokio_stream::Stream<Item = Result<ene_ai::message::LlmResponseChunk, E>> + Unpin,
{
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut stream = stream;
        while stream.next().await.is_some() {}
    })
    .await
    .expect("stream must end after the serving process dies");
}

/// Two providers from *different* plugin processes declaring the same
/// `Gpu{0}` must share one budget: the second stream waits while the first
/// is in flight and is admitted once the first is dropped.
#[tokio::test]
async fn same_class_providers_share_one_budget_across_plugins() {
    let admission = std::sync::Arc::new(ResourceClassAdmission::new(&[]));
    let class = ResourceClass::Gpu { device: 0 };

    let (socket_a, server_a, conn_a) = connect_mock("share-a", class).await;
    let (socket_b, server_b, conn_b) = connect_mock("share-b", class).await;
    let factory_a = mock_factory(conn_a, class, &admission);
    let factory_b = mock_factory(conn_b, class, &admission);
    let task = ene_ai::TaskRef::default();

    let stream_a = start_hold_stream(&factory_a, &task)
        .await
        .expect("first provider admitted");

    // The second provider's request must not be sent while the class budget
    // is held — it waits at the admission gate.
    let second = tokio::spawn({
        let task = task.clone();
        async move { start_hold_stream(&factory_b, &task).await }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !second.is_finished(),
        "second same-class provider must wait for the shared budget"
    );

    // Dropping the first stream releases the permit; the queued request is
    // then admitted.
    drop(stream_a);
    let stream_b = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("queued provider must be admitted after the permit is released")
        .expect("join")
        .expect("second provider admitted");
    drop(stream_b);

    server_a.abort();
    server_b.abort();
    cleanup_path(&socket_a);
    cleanup_path(&socket_b);
}

/// Distinct GPU devices are independent budgets.
#[tokio::test]
async fn distinct_devices_are_independent_across_plugins() {
    let admission = std::sync::Arc::new(ResourceClassAdmission::new(&[]));
    let (socket_a, server_a, conn_a) =
        connect_mock("device-a", ResourceClass::Gpu { device: 0 }).await;
    let (socket_b, server_b, conn_b) =
        connect_mock("device-b", ResourceClass::Gpu { device: 1 }).await;
    let factory_a = mock_factory(conn_a, ResourceClass::Gpu { device: 0 }, &admission);
    let factory_b = mock_factory(conn_b, ResourceClass::Gpu { device: 1 }, &admission);
    let task = ene_ai::TaskRef::default();

    let stream_a = start_hold_stream(&factory_a, &task)
        .await
        .expect("device 0 admitted");
    let stream_b =
        tokio::time::timeout(Duration::from_secs(2), start_hold_stream(&factory_b, &task))
            .await
            .expect("device 1 must not wait on device 0's budget")
            .expect("device 1 admitted");
    drop(stream_a);
    drop(stream_b);

    server_a.abort();
    server_b.abort();
    cleanup_path(&socket_a);
    cleanup_path(&socket_b);
}

/// A configured budget can admit more than one concurrent job per class.
#[tokio::test]
async fn configured_budget_raises_concurrency_per_class() {
    let admission = std::sync::Arc::new(ResourceClassAdmission::new(&[ResourceClassBudget {
        class: ResourceClass::Gpu { device: 0 },
        permits: Some(2),
        queue_depth: Some(0),
    }]));
    let class = ResourceClass::Gpu { device: 0 };
    let (socket_a, server_a, conn_a) = connect_mock("budget-a", class).await;
    let (socket_b, server_b, conn_b) = connect_mock("budget-b", class).await;
    let (socket_c, server_c, conn_c) = connect_mock("budget-c", class).await;
    let factory_a = mock_factory(conn_a, class, &admission);
    let factory_b = mock_factory(conn_b, class, &admission);
    let factory_c = mock_factory(conn_c, class, &admission);
    let task = ene_ai::TaskRef::default();

    let stream_a = start_hold_stream(&factory_a, &task)
        .await
        .expect("first permit");
    let stream_b =
        tokio::time::timeout(Duration::from_secs(2), start_hold_stream(&factory_b, &task))
            .await
            .expect("second permit")
            .expect("second provider admitted");

    // Both permits are held; a third provider fails fast (queue_depth 0).
    let result = start_hold_stream(&factory_c, &task).await;
    let Err(err) = result else {
        panic!("third provider must be rejected");
    };
    assert!(matches!(err, ene_ai::error::LlmProviderError::Busy { .. }));

    drop(stream_a);
    drop(stream_b);
    server_a.abort();
    server_b.abort();
    server_c.abort();
    cleanup_path(&socket_a);
    cleanup_path(&socket_b);
    cleanup_path(&socket_c);
}

/// A transport failure (the serving process dying) must release the held
/// class permit through the reader failure path.
#[tokio::test]
async fn permit_released_when_serving_task_dies() {
    let admission = std::sync::Arc::new(ResourceClassAdmission::new(&[]));
    let class = ResourceClass::Gpu { device: 0 };
    let (socket_a, server_a, conn_a) = connect_mock("crash-a", class).await;
    let ping_conn = std::sync::Arc::clone(&conn_a);
    let factory_a = mock_factory(conn_a, class, &admission);
    let task = ene_ai::TaskRef::default();
    let stream_a = start_hold_stream(&factory_a, &task)
        .await
        .expect("first provider admitted");

    // The mock must be alive with the stream genuinely in flight — a ping
    // round-trip proves the connection did not die on a decode error.
    ping_conn
        .ping()
        .await
        .expect("mock connection alive mid-stream");

    server_a.abort();
    drain_until_end(stream_a).await;

    // The permit must be free again: a fresh provider on a fresh connection
    // is admitted promptly instead of waiting forever.
    let (socket_b, server_b, conn_b) = connect_mock("crash-b", class).await;
    let factory_b = mock_factory(conn_b, class, &admission);
    let stream_b =
        tokio::time::timeout(Duration::from_secs(5), start_hold_stream(&factory_b, &task))
            .await
            .expect("permit must be released after the serving process dies")
            .expect("fresh provider admitted");
    drop(stream_b);

    server_b.abort();
    cleanup_path(&socket_a);
    cleanup_path(&socket_b);
}

/// The child-mode entry point for the SIGKILL test: runs a mock server in a
/// real subprocess (this test binary re-executed with `--exact`), which the
/// parent kills with SIGKILL. No-op when the env var is absent.
#[tokio::test]
async fn mock_server_child() {
    let Ok(socket) = std::env::var("ENE_ADMISSION_MOCK_SOCKET") else {
        return;
    };
    let class = std::env::var("ENE_ADMISSION_MOCK_CLASS")
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(ResourceClass::Cpu);
    let (shutdown_tx, shutdown_rx) = watch::channel(());
    // The sender outlives the server so connections stay open until this
    // process is SIGKILLed by the parent test.
    let _sender = shutdown_tx;
    run_admission_server(PathBuf::from(socket), class, shutdown_rx).await;
    loop {
        tokio::time::sleep(Duration::from_hours(1)).await;
    }
}

/// A real SIGKILL of the serving plugin process must release the held class
/// permit; a queued request then proceeds (and fails on the dead connection)
/// instead of waiting on a leaked permit.
#[cfg(unix)]
#[tokio::test]
async fn permit_released_when_serving_process_is_sigkilled() {
    let socket_path = test_socket_path("sigkill");
    let class = ResourceClass::Gpu { device: 0 };
    let child = spawn_child_mock_server(&socket_path, class);

    // Wait for the child to bind its socket.
    tokio::time::timeout(Duration::from_secs(5), async {
        while !socket_path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("child mock server must bind");

    let admission = std::sync::Arc::new(ResourceClassAdmission::new(&[]));
    let conn = IpcPluginConnection::connect(
        &socket_path,
        SandboxConfigData::default(),
        None,
        None,
        TEST_HANDSHAKE_TIMEOUT,
        TEST_MAX_CONCURRENT,
    )
    .await
    .expect("handshake with child mock server");
    let conn = std::sync::Arc::new(conn);
    let ping_conn = std::sync::Arc::clone(&conn);
    let factory = mock_factory(conn, class, &admission);
    let factory = std::sync::Arc::new(factory);
    let task = ene_ai::TaskRef::default();
    let stream_a = start_hold_stream(&factory, &task)
        .await
        .expect("first provider admitted");
    ping_conn.ping().await.expect("child mock alive mid-stream");

    // A second request on the same class queues on the held permit.
    let queued = tokio::spawn({
        let task = task.clone();
        let factory = std::sync::Arc::clone(&factory);
        async move { start_hold_stream(&factory, &task).await }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !queued.is_finished(),
        "second same-class request must wait for the held permit"
    );

    kill_child(child);
    drain_until_end(stream_a).await;

    // The queued request must now pass the admission gate (the permit was
    // released). It then fails on the dead connection, or — if the write was
    // buffered before the socket closed — returns a stream that dies moments
    // later; either way it cleared the gate, which is the property under
    // test (a leaked permit would still be blocking here).
    let _outcome = tokio::time::timeout(Duration::from_secs(15), queued)
        .await
        .expect("queued request must proceed once the permit is released")
        .expect("join");

    cleanup_path(&socket_path);
}

#[cfg(unix)]
fn spawn_child_mock_server(socket_path: &Path, class: ResourceClass) -> Child {
    let exe = std::env::current_exe().expect("test binary path");
    let class_json = serde_json::to_string(&class).expect("class serializes");
    Command::new(exe)
        .args(["--exact", "mock_server_child", "--nocapture"])
        .env("ENE_ADMISSION_MOCK_SOCKET", socket_path)
        .env("ENE_ADMISSION_MOCK_CLASS", class_json)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn self as mock server")
}

#[cfg(unix)]
fn kill_child(mut child: Child) {
    child.kill().expect("SIGKILL the mock server child");
    child.wait().expect("reap the mock server child");
}
