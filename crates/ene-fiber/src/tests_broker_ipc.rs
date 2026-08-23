use crate::{Broker, BrokerServer, FiberUid};
use base64::{Engine, engine::general_purpose::STANDARD};
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

    #[cfg(unix)]
    let socket = std::path::PathBuf::from(&path);
    drop(server);
    #[cfg(unix)]
    assert!(
        !socket.exists(),
        "broker socket must be removed when the server is dropped"
    );
}

#[tokio::test]
async fn broker_client_round_trips_binary_files() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("image.bin"), [0_u8, 1, 2, 255]).unwrap();
    let mut broker = Broker::new(dir.path().to_path_buf());
    let uid = FiberUid::new();
    broker.grant(uid, "fs.read");
    broker.grant(uid, "fs.write");

    let server = BrokerServer::bind(
        Arc::new(parking_lot::Mutex::new(broker)),
        uid,
        "r-broker-binary",
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

    let read = client
        .call(BrokerRequest::FsReadBytes {
            path: dir.path().join("image.bin").display().to_string(),
        })
        .await
        .unwrap();
    let BrokerResponse::FsReadBytesOk { bytes_base64 } = read else {
        unreachable!("expected binary read response, got {read:?}")
    };
    assert_eq!(STANDARD.decode(bytes_base64).unwrap(), [0_u8, 1, 2, 255]);

    let output = dir.path().join("copy.bin");
    let written = client
        .call(BrokerRequest::FsWriteBytes {
            path: output.display().to_string(),
            bytes_base64: STANDARD.encode([9_u8, 0, 255]),
        })
        .await
        .unwrap();
    assert_eq!(written, BrokerResponse::FsWriteBytesOk);
    assert_eq!(std::fs::read(&output).unwrap(), [9_u8, 0, 255]);
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
    std::fs::write(dir.path().join("a.txt"), "hit\nhit\nhit\n").unwrap();
    std::fs::write(dir.path().join("b.txt"), "hit\nhit\nhit\n").unwrap();
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
            query: "hit".to_owned(),
            regex: false,
            case_insensitive: false,
            include: Some("*.txt".to_owned()),
            context_lines: 0,
            count: false,
            max: 2,
        })
        .await
        .unwrap();
    let BrokerResponse::FsSearchOk { matches } = response else {
        panic!("expected search response, got {response:?}");
    };
    let rows = matches.as_array().expect("normalized match rows");
    assert_eq!(rows.len(), 2, "max must be global across files");
    assert!(rows.iter().all(|row| row.get("text").is_some()));
    assert!(rows.iter().all(|row| row.get("type").is_none()));

    let response = client
        .call(BrokerRequest::FsSearch {
            path: dir.path().display().to_string(),
            query: "hit".to_owned(),
            regex: false,
            case_insensitive: false,
            include: Some("*.txt".to_owned()),
            context_lines: 0,
            count: true,
            max: 1,
        })
        .await
        .unwrap();
    let BrokerResponse::FsSearchOk { matches } = response else {
        panic!("expected count response, got {response:?}");
    };
    assert_eq!(matches["total"], 6);
    let files = matches["files"].as_array().expect("per-file counts");
    assert_eq!(files.len(), 2);
    assert!(files.iter().all(|row| row.get("count").is_some()));
    assert!(files.iter().all(|row| row.get("text").is_none()));
}
