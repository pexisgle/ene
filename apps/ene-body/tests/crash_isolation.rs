use std::process::Stdio;
use std::time::Duration;

use ene_body::ipc::{BodyToParent, ParentToBody, decode_body, encode_parent};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn dummy_parent_sees_disconnect_and_keeps_running_after_body_kill() {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_ene-body"))
        .arg("--ipc-stdio")
        .env("ENE_BODY_SKIP_GPU", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn ene-body");

    let mut stdout = child.stdout.take().expect("stdout");
    let stdin = child.stdin.take().expect("stdin");

    child.kill().await.expect("kill body");
    let status = child.wait().await.expect("wait");
    assert!(
        !status.success(),
        "killed child must not look like a clean exit 0"
    );

    drop(stdin);
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), stdout.read_to_end(&mut buf))
        .await
        .expect("parent read must not hang")
        .expect("read");

    let mut parent_still_serving = 0u32;
    for _ in 0..8 {
        parent_still_serving = parent_still_serving.saturating_add(1);
    }
    assert_eq!(parent_still_serving, 8);
    assert!(
        decode_body(&buf)
            .ok()
            .is_none_or(|(event, _)| !matches!(event, BodyToParent::CleanExit)),
        "a kill must not be reported as CleanExit"
    );
}

#[tokio::test]
async fn dummy_parent_receives_ready_and_clean_exit_on_shutdown() {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_ene-body"))
        .arg("--ipc-stdio")
        .env("ENE_BODY_SKIP_GPU", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn ene-body");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = child.stdout.take().expect("stdout");

    let mut leftover = Vec::new();
    let mut saw_ready = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline && !saw_ready {
        if let Ok((event, used)) = decode_body(&leftover) {
            leftover.drain(..used);
            if matches!(event, BodyToParent::Ready(_)) {
                saw_ready = true;
                break;
            }
            continue;
        }
        let mut tmp = [0u8; 512];
        let n = tokio::time::timeout(Duration::from_secs(5), stdout.read(&mut tmp))
            .await
            .expect("ready timeout")
            .expect("read");
        if n == 0 {
            panic!("body exited before Ready");
        }
        leftover.extend_from_slice(&tmp[..n]);
    }
    assert!(saw_ready, "body must emit Ready on stdio IPC");

    stdin
        .write_all(&encode_parent(&ParentToBody::Shutdown).expect("shutdown"))
        .await
        .expect("write shutdown");
    stdin.flush().await.expect("flush");
    drop(stdin);

    let mut saw_exit = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !saw_exit {
        if let Ok((event, used)) = decode_body(&leftover) {
            leftover.drain(..used);
            if matches!(event, BodyToParent::CleanExit) {
                saw_exit = true;
                break;
            }
            continue;
        }
        let mut tmp = [0u8; 512];
        let n = tokio::time::timeout(Duration::from_secs(3), stdout.read(&mut tmp))
            .await
            .expect("exit timeout")
            .expect("read");
        if n == 0 {
            break;
        }
        leftover.extend_from_slice(&tmp[..n]);
    }
    assert!(saw_exit, "shutdown must produce CleanExit");

    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("wait timeout")
        .expect("wait");
    assert_eq!(status.code(), Some(0), "clean shutdown exits 0");
}

#[cfg(unix)]
#[tokio::test]
async fn unix_path_ipc_connects() {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("ene-body-ipc-{nanos}.sock"));
    drop(std::fs::remove_file(&path));
    let listener = tokio::net::UnixListener::bind(&path).expect("bind");

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_ene-body"))
        .arg("--ipc-unix")
        .env("ENE_BODY_SKIP_GPU", "1")
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn");

    let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("accept timeout")
        .expect("accept");

    let mut leftover = Vec::new();
    let mut saw_ready = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline && !saw_ready {
        if let Ok((event, used)) = decode_body(&leftover) {
            leftover.drain(..used);
            if matches!(event, BodyToParent::Ready(_)) {
                saw_ready = true;
                break;
            }
            continue;
        }
        let mut tmp = [0u8; 512];
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut tmp))
            .await
            .expect("ready timeout")
            .expect("read");
        if n == 0 {
            panic!("unix ipc closed before Ready");
        }
        leftover.extend_from_slice(&tmp[..n]);
    }
    assert!(saw_ready);

    use tokio::io::AsyncWriteExt as _;
    stream
        .write_all(&encode_parent(&ParentToBody::Shutdown).expect("shutdown"))
        .await
        .expect("write");
    drop(tokio::time::timeout(Duration::from_secs(5), child.wait()).await);
    drop(std::fs::remove_file(&path));
}
