use std::net::{SocketAddr, TcpListener};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Handle;

/// Serve one surface-history response on an ephemeral local port.
pub(crate) fn spawn(rt: &Handle) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind history test server");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let addr = listener.local_addr().expect("history test server address");
    rt.spawn(async move {
        let deadline = Instant::now() + Duration::from_secs(2);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for history request"
                    );
                    tokio::task::yield_now().await;
                }
                Err(err) => panic!("accept failed: {err}"),
            }
        };
        stream.set_nonblocking(true).expect("nonblocking stream");
        let mut stream = tokio::net::TcpStream::from_std(stream).expect("convert accepted stream");
        serve_one(&mut stream).await.expect("serve history");
    });
    addr
}

async fn serve_one(stream: &mut tokio::net::TcpStream) -> Result<(), std::io::Error> {
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    while !request.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await?;
        request.push(byte[0]);
    }
    assert!(request.starts_with(b"GET /api/v1/sessions/new-session/history"));
    let body = br#"{"messages":[{"seq":1,"role":"assistant","text":"new"}],"depth":"surface"}"#;
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await?;
    stream.write_all(body).await?;
    stream.shutdown().await?;
    Ok(())
}
