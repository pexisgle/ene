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
    )
    .unwrap();
    let path = server.socket_path().to_string_lossy().into_owned();

    let mut client = BrokerClient::from_path(&path).await.unwrap();
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

    drop(server);
}
