use crate::{Broker, BrokerServer, FiberUid};
use ene_plugin_ipc::{BrokerClient, BrokerRequest, BrokerResponse, BrokerSession};
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn broker_session_authenticates_once_and_reuses_the_connection() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("input.txt"), "alpha\nbeta\n").unwrap();
    let mut broker = Broker::new(dir.path().to_path_buf());
    let uid = FiberUid::new();
    broker.grant(uid, "fs.read");

    let server = BrokerServer::bind(
        Arc::new(parking_lot::Mutex::new(broker)),
        uid,
        "r-broker-session",
        "test-token",
    )
    .unwrap();
    let session = BrokerSession::new(BrokerClient::from_path(server.endpoint()).await.unwrap());
    assert!(!session.is_authenticated());

    let first = session
        .call(
            "test-token",
            BrokerRequest::FsRead {
                path: dir.path().join("input.txt").display().to_string(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        first,
        BrokerResponse::FsReadOk {
            text: "alpha\nbeta\n".to_owned()
        }
    );
    assert!(session.is_authenticated());

    let second = session
        .call(
            "ignored",
            BrokerRequest::FsWrite {
                path: dir.path().join("output.txt").display().to_string(),
                text: "nope".to_owned(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(second, BrokerResponse::Error { .. }));
}

#[tokio::test]
async fn broker_session_returns_hello_rejection_without_caching_it() {
    let dir = TempDir::new().unwrap();
    let server = BrokerServer::bind(
        Arc::new(parking_lot::Mutex::new(Broker::new(
            dir.path().to_path_buf(),
        ))),
        FiberUid::new(),
        "r-broker-session-reject",
        "expected-token",
    )
    .unwrap();
    let session = BrokerSession::new(BrokerClient::from_path(server.endpoint()).await.unwrap());

    let rejected = session
        .call(
            "wrong",
            BrokerRequest::FsRead {
                path: "input.txt".to_owned(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(rejected, BrokerResponse::Error { .. }));
    assert!(!session.is_authenticated());
}
