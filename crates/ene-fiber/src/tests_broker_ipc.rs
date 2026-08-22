use crate::{Broker, BrokerServer, FiberUid};
use ene_plugin_ipc::{BrokerClient, BrokerRequest, BrokerResponse};
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn broker_client_round_trips_fs_read_and_denies_undeclared_ops() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("input.txt"), "ok").unwrap();
    let mut broker = Broker::new(dir.path().to_path_buf());
    let uid = FiberUid::new();
    broker.grant(uid, "fs.read");

    let server = BrokerServer::bind(
        Arc::new(parking_lot::Mutex::new(broker)),
        uid,
        "r-broker-roundtrip",
        "test-token",
    )
    .unwrap();
    let path = server.endpoint().to_owned();

    let mut client = BrokerClient::from_path(&path).await.unwrap();
    let hello = client
        .call(BrokerRequest::Hello {
            token: "test-token".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(hello, BrokerResponse::HelloOk);

    let read = client
        .call(BrokerRequest::FsRead {
            path: dir.path().join("input.txt").display().to_string(),
        })
        .await
        .unwrap();
    assert_eq!(
        read,
        BrokerResponse::FsReadOk {
            text: "ok".to_owned()
        }
    );

    let denied = client
        .call(BrokerRequest::FsWrite {
            path: dir.path().join("output.txt").display().to_string(),
            text: "nope".to_owned(),
        })
        .await
        .unwrap();
    assert!(matches!(denied, BrokerResponse::Error { .. }));
    assert!(!dir.path().join("output.txt").exists());

    let socket = std::path::PathBuf::from(&path);
    drop(server);
    #[cfg(unix)]
    assert!(
        !socket.exists(),
        "broker socket must be removed when the server is dropped"
    );
}

#[tokio::test]
async fn broker_client_rejects_bad_hello() {
    let dir = TempDir::new().unwrap();
    let server = BrokerServer::bind(
        Arc::new(parking_lot::Mutex::new(Broker::new(
            dir.path().to_path_buf(),
        ))),
        FiberUid::new(),
        "r-broker-bad-token",
        "expected-token",
    )
    .unwrap();
    let mut client = BrokerClient::from_path(server.endpoint()).await.unwrap();

    let rejected = client
        .call(BrokerRequest::Hello {
            token: "wrong".to_owned(),
        })
        .await
        .unwrap();
    assert!(matches!(rejected, BrokerResponse::Error { .. }));
}

#[tokio::test]
async fn broker_client_round_trips_fs_search() {
    #![cfg_attr(test, expect(clippy::panic, reason = "tests fail fast"))]
    #![cfg_attr(
        test,
        expect(clippy::print_stderr, reason = "test skip must remain visible")
    )]
    if std::process::Command::new("rg")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping search broker test because rg is not installed");
        return;
    }
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("input.txt"), "alpha\nbeta\n").unwrap();
    let mut broker = Broker::new(dir.path().to_path_buf());
    let uid = FiberUid::new();
    broker.grant(uid, "fs.search");

    let server = BrokerServer::bind(
        Arc::new(parking_lot::Mutex::new(broker)),
        uid,
        "r-broker-search",
        "test-token",
    )
    .unwrap();
    let mut client = BrokerClient::from_path(server.endpoint()).await.unwrap();
    client
        .call(BrokerRequest::Hello {
            token: "test-token".to_owned(),
        })
        .await
        .unwrap();

    let response = client
        .call(BrokerRequest::FsSearch {
            path: dir.path().display().to_string(),
            query: "beta".to_owned(),
            regex: false,
            case_insensitive: false,
            include: Some("*.txt".to_owned()),
            context_lines: 0,
            count: false,
            max: 10,
        })
        .await
        .unwrap();
    let BrokerResponse::FsSearchOk { matches } = response else {
        panic!("expected search response, got {response:?}");
    };
    assert!(matches.is_array());
}

#[test]
fn broker_rpc_timeout_is_machine_classified() {
    assert_eq!(
        crate::broker_ipc::timeout_response(std::time::Duration::from_secs(30)),
        BrokerResponse::Error {
            code: ene_plugin_ipc::BrokerErrorCode::Timeout,
            message: "broker operation timed out after 30s".to_owned(),
        }
    );
}
