//! The real Microsoft service is never contacted.
#![expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ene_plugin::TtsPlugin as _;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{WebSocketStream, accept_async};

use crate::audio::decode_mp3;
use crate::plugin::EdgeTtsPlugin;

/// Real Edge output captured from the service (Layer III, 24 kHz mono,
/// 48 kbps CBR); 9 decodable frames = `5_184` samples.
const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/edge-tts-tone.mp3");

const KIND: &str = "edge-tts";

fn audio_frame(payload: &[u8]) -> Vec<u8> {
    let header = b"X-RequestId:test\r\nContent-Type:audio/mpeg\r\nPath:audio\r\n";
    let mut frame = Vec::with_capacity(2 + header.len() + payload.len());
    frame.extend_from_slice(&(header.len() as u16).to_be_bytes());
    frame.extend_from_slice(header);
    frame.extend_from_slice(payload);
    frame
}

fn text_frame(path: &str) -> String {
    format!("X-RequestId:test\r\nPath:{path}\r\n\r\n{{}}")
}

async fn spawn_server<F, Fut>(serve: F) -> SocketAddr
where
    F: FnOnce(WebSocketStream<TcpStream>) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let ws = accept_async(stream).await.expect("handshake");
        serve(ws).await;
    });
    addr
}

async fn next_text(
    stream: &mut (impl StreamExt<Item = Result<Message, WsError>> + Unpin),
) -> Result<String, String> {
    match stream.next().await {
        Some(Ok(Message::Text(text))) => Ok(text.to_string()),
        Some(Ok(_)) => Err("expected a text frame".to_string()),
        Some(Err(e)) => Err(e.to_string()),
        None => Err("stream closed".to_string()),
    }
}

async fn serve_chunks(ws: WebSocketStream<TcpStream>, chunk_count: usize) {
    let (mut sink, mut stream) = ws.split();
    let speech = next_text(&mut stream).await.expect("speech.config");
    assert!(speech.contains("Path:speech.config"), "{speech}");
    assert!(
        speech.contains("audio-24khz-48kbitrate-mono-mp3"),
        "{speech}"
    );
    for _ in 0..chunk_count {
        let ssml = next_text(&mut stream).await.expect("ssml request");
        assert!(ssml.contains("Path:ssml"), "{ssml}");
        assert!(ssml.contains("<speak version='1.0'"), "{ssml}");

        sink.send(Message::text(text_frame("turn.start")))
            .await
            .expect("send");
        sink.send(Message::binary(audio_frame(FIXTURE)))
            .await
            .expect("send");
        sink.send(Message::text(text_frame("turn.end")))
            .await
            .expect("send");
    }
}

async fn synthesize_with_endpoint(
    addr: &SocketAddr,
    text: &str,
) -> Result<Vec<u8>, ene_plugin::PluginError> {
    let _serial = crate::broker::tests::with_broker().await;
    let config = json!({"endpoint_url": format!("ws://{addr}"), "max_retries": 3});
    tokio::time::timeout(
        Duration::from_secs(10),
        EdgeTtsPlugin::default().synthesize(
            KIND,
            config,
            text.to_string(),
            "ja-JP-NanamiNeural".to_string(),
            "wav".to_string(),
        ),
    )
    .await
    .expect("no timeout")
}

#[tokio::test]
async fn multi_chunk_synthesis_round_trips_wav() {
    let addr = spawn_server(|ws| serve_chunks(ws, 2)).await;

    // Two chunks: the first fills the 4096-byte budget, the second is the
    // remainder. Both are served over the same connection.
    let text = format!("{} a", "x".repeat(4096));

    let wav = synthesize_with_endpoint(&addr, &text)
        .await
        .expect("synthesis");

    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    let expected_pcm = decode_mp3(FIXTURE).expect("fixture decodes").pcm.len() * 2;
    let reader = hound::WavReader::new(std::io::Cursor::new(&wav)).expect("valid wav");
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 24_000);
    assert_eq!(spec.channels, 1);
    assert_eq!(spec.bits_per_sample, 32);
    assert_eq!(spec.sample_format, hound::SampleFormat::Float);
    let sample_count = reader.into_samples::<f32>().count();
    assert_eq!(sample_count, expected_pcm);
}

#[tokio::test]
async fn reconnects_after_connection_drop() {
    let connections = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("address");
    let count = Arc::clone(&connections);
    tokio::spawn(async move {
        for connection in 0..2 {
            let (stream, _) = listener.accept().await.expect("accept");
            count.fetch_add(1, Ordering::SeqCst);
            let ws = accept_async(stream).await.expect("handshake");
            if connection == 0 {
                // Drop without sending turn.end; the client must retry.
                continue;
            }
            serve_chunks(ws, 1).await;
        }
    });

    let wav = synthesize_with_endpoint(&addr, "こんにちは")
        .await
        .expect("synthesis after retry");

    assert_eq!(connections.load(Ordering::SeqCst), 2);
    assert_eq!(&wav[..4], b"RIFF");
}

#[tokio::test]
async fn turn_end_without_audio_is_no_audio_error() {
    async fn serve_no_audio(ws: WebSocketStream<TcpStream>) {
        let (mut sink, mut stream) = ws.split();
        let _ = next_text(&mut stream).await.expect("speech.config");
        let _ = next_text(&mut stream).await.expect("ssml request");
        sink.send(Message::text(text_frame("turn.start")))
            .await
            .expect("send");
        sink.send(Message::text(text_frame("turn.end")))
            .await
            .expect("send");
    }
    let addr = spawn_server(serve_no_audio).await;

    let err = synthesize_with_endpoint(&addr, "こんにちは")
        .await
        .expect_err("no audio");
    assert!(err.to_string().contains("no audio"), "{err}");
}

#[tokio::test]
async fn http_404_is_rejected_without_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        use tokio::io::AsyncWriteExt;
        stream
            .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
            .await
            .expect("write");
    });

    let err = synthesize_with_endpoint(&addr, "こんにちは")
        .await
        .expect_err("not found");
    assert!(err.to_string().contains("404"), "{err}");
}

#[tokio::test]
async fn http_429_is_retried() {
    let connections = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("address");
    let count = Arc::clone(&connections);
    tokio::spawn(async move {
        for connection in 0..2 {
            let (mut stream, _) = listener.accept().await.expect("accept");
            count.fetch_add(1, Ordering::SeqCst);
            if connection == 0 {
                use tokio::io::AsyncWriteExt;
                stream
                    .write_all(b"HTTP/1.1 429 Too Many Requests\r\ncontent-length: 0\r\n\r\n")
                    .await
                    .expect("write");
                continue;
            }
            let ws = accept_async(stream).await.expect("handshake");
            serve_chunks(ws, 1).await;
        }
    });

    let wav = synthesize_with_endpoint(&addr, "こんにちは")
        .await
        .expect("synthesis after 429 retry");
    assert_eq!(connections.load(Ordering::SeqCst), 2);
    assert_eq!(&wav[..4], b"RIFF");
}

#[tokio::test]
async fn http_403_retries_after_clock_skew_correction() {
    let connections = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("address");
    let count = Arc::clone(&connections);
    tokio::spawn(async move {
        for connection in 0..2 {
            let (mut stream, _) = listener.accept().await.expect("accept");
            count.fetch_add(1, Ordering::SeqCst);
            if connection == 0 {
                use tokio::io::AsyncWriteExt;
                let date = chrono::Utc::now()
                    .format("%a, %d %b %Y %H:%M:%S GMT")
                    .to_string();
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 403 Forbidden\r\ndate: {date}\r\ncontent-length: 0\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await
                    .expect("write");
                continue;
            }
            let ws = accept_async(stream).await.expect("handshake");
            serve_chunks(ws, 1).await;
        }
    });

    let wav = synthesize_with_endpoint(&addr, "こんにちは")
        .await
        .expect("synthesis after 403 retry");
    assert_eq!(connections.load(Ordering::SeqCst), 2);
    assert_eq!(&wav[..4], b"RIFF");
}

#[tokio::test]
async fn rejects_wrong_kind_format_and_empty_text() {
    let plugin = EdgeTtsPlugin::default();
    let config = json!({});

    let err = plugin
        .synthesize(
            "other",
            json!({}),
            "hi".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("wrong kind");
    assert!(err.to_string().contains("not supported"), "{err}");

    let err = plugin
        .synthesize(
            KIND,
            config.clone(),
            "hi".to_string(),
            String::new(),
            "mp3".to_string(),
        )
        .await
        .expect_err("wrong format");
    assert!(err.to_string().contains("format"), "{err}");

    let err = plugin
        .synthesize(
            KIND,
            config,
            "  \t ".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("empty text");
    assert!(err.to_string().contains("no speakable text"), "{err}");
}

#[test]
fn capability_spec_matches_provider_kind() {
    let spec = EdgeTtsPlugin::tts_spec();
    assert_eq!(spec.kind, KIND);
    assert_eq!(spec.formats, vec!["wav"]);
}
