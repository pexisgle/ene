//! Fail-closed liveness probe for a serving local Host.
//!
//! A crash can leave `host-runtime.json` behind while some unrelated process
//! reuses the old port, so a bare TCP connect proves nothing (persistence
//! recovery: the file's existence is never proof of liveness). The probe
//! completes the local WSS upgrade instead: it verifies the served
//! certificate against the trusted Host pin over TLS and presents the
//! current local token and startup generation, which only the real serving
//! Host accepts. It sends no business frames and writes nothing — first
//! trust and pairing stay with `begin_connect`.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const RESPONSE_CAP: usize = 8 * 1024;

fn base64_key(bytes: &[u8; 16]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(24);
    for chunk in bytes.chunks(3) {
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        let packed = (u32::from(chunk[0]) << 16) | (u32::from(second) << 8) | u32::from(third);
        let indexes = [
            (packed >> 18) & 0x3f,
            (packed >> 12) & 0x3f,
            (packed >> 6) & 0x3f,
            packed & 0x3f,
        ];
        match chunk.len() {
            3 => {
                for index in indexes {
                    out.push(ALPHABET[index as usize] as char);
                }
            }
            2 => {
                for index in [indexes[0], indexes[1], indexes[2]] {
                    out.push(ALPHABET[index as usize] as char);
                }
                out.push('=');
            }
            _ => {
                for index in [indexes[0], indexes[1]] {
                    out.push(ALPHABET[index as usize] as char);
                }
                out.push_str("==");
            }
        }
    }
    out
}

pub(crate) fn probe_serving_host(data_dir: &Path) -> bool {
    let Ok(runtime) = crate::runtime_info::load_host_runtime(data_dir) else {
        return false;
    };
    let Some(port) = runtime.local_port() else {
        return false;
    };
    let Ok(config) = crate::host_pin::pinned_tls_config(&runtime.host_pin) else {
        return false;
    };
    let address = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    let Ok(tcp) = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) else {
        return false;
    };
    if tcp.set_read_timeout(Some(IO_TIMEOUT)).is_err()
        || tcp.set_write_timeout(Some(IO_TIMEOUT)).is_err()
    {
        return false;
    }
    let server_name = rustls::pki_types::ServerName::IpAddress(
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST).into(),
    );
    let Ok(connection) = rustls::ClientConnection::new(Arc::new(config), server_name) else {
        return false;
    };
    let mut stream = rustls::StreamOwned::new(connection, tcp);
    let request = format!(
        "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\nauthorization: Bearer {}\r\nx-ene-startup-generation: {}\r\n\r\n",
        base64_key(uuid::Uuid::new_v4().as_bytes()),
        runtime.local_token,
        runtime.startup_generation
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = Vec::new();
    let mut byte = [0_u8; 1];
    while !response.windows(4).any(|window| window == b"\r\n\r\n") {
        if response.len() >= RESPONSE_CAP {
            return false;
        }
        match stream.read(&mut byte) {
            Ok(0) => return false,
            Ok(_) => response.push(byte[0]),
            Err(_) => return false,
        }
    }
    let head = String::from_utf8_lossy(&response);
    head.lines()
        .next()
        .is_some_and(|status_line| status_line.contains(" 101 "))
}
