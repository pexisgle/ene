use std::process::Stdio;
use std::time::Duration;

use ene_body::ipc::{BodyToParent, IpcError, ParentToBody, decode_body, encode_parent};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

#[expect(clippy::expect_used, clippy::panic, reason = "test fixture helper")]
async fn read_until<R, F>(
    reader: &mut R,
    frame: &mut Vec<u8>,
    deadline: tokio::time::Instant,
    predicate: F,
    closed: &str,
) where
    R: AsyncRead + Unpin,
    F: Fn(&BodyToParent) -> bool,
{
    loop {
        match decode_body(frame) {
            Ok((event, used)) => {
                frame.drain(..used);
                if predicate(&event) {
                    return;
                }
            }
            Err(IpcError::Truncated { .. }) => {}
            Err(error) => panic!("invalid body frame: {error}"),
        }
        let mut chunk = [0_u8; 512];
        let count = tokio::time::timeout_at(deadline, reader.read(&mut chunk))
            .await
            .expect("body read timed out")
            .expect("body read failed");
        if count == 0 {
            // End of stream still leaves whatever the peer already wrote sitting
            // in the buffer, and readiness can report the close before the last
            // chunk is drained, so decode the remainder before calling it closed.
            if let Ok((event, used)) = decode_body(frame) {
                frame.drain(..used);
                if predicate(&event) {
                    return;
                }
            }
            panic!("{closed}");
        }
        frame.extend_from_slice(&chunk[..count]);
    }
}

#[tokio::test]
async fn dummy_parent_receives_ready_and_clean_exit_on_shutdown() {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_ene-body"))
        .arg("--ipc-stdio")
        .env("ENE_BODY_SKIP_GPU", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn ene-body");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = child.stdout.take().expect("stdout");
    let mut frame = Vec::new();
    read_until(
        &mut stdout,
        &mut frame,
        tokio::time::Instant::now() + Duration::from_secs(8),
        |event| matches!(event, BodyToParent::Ready(_)),
        "body exited before Ready",
    )
    .await;

    stdin
        .write_all(&encode_parent(&ParentToBody::Shutdown).expect("shutdown"))
        .await
        .expect("write shutdown");
    stdin.flush().await.expect("flush");
    drop(stdin);

    read_until(
        &mut stdout,
        &mut frame,
        tokio::time::Instant::now() + Duration::from_secs(5),
        |event| matches!(event, BodyToParent::CleanExit),
        "body exited before CleanExit",
    )
    .await;

    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("wait timeout")
        .expect("wait");
    assert!(status.success(), "clean shutdown exits successfully");
}

#[cfg(unix)]
#[tokio::test]
async fn unix_path_ipc_connects() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ene-body-ipc.sock");
    let listener = tokio::net::UnixListener::bind(&path).expect("bind");

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_ene-body"))
        .arg("--ipc-unix")
        .env("ENE_BODY_SKIP_GPU", "1")
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn");

    let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("accept timeout")
        .expect("accept");

    let mut frame = Vec::new();
    read_until(
        &mut stream,
        &mut frame,
        tokio::time::Instant::now() + Duration::from_secs(8),
        |event| matches!(event, BodyToParent::Ready(_)),
        "unix ipc closed before Ready",
    )
    .await;

    stream
        .write_all(&encode_parent(&ParentToBody::Shutdown).expect("shutdown"))
        .await
        .expect("write");
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("wait timeout")
        .expect("wait");
    assert!(status.success(), "unix shutdown exits successfully");
}
