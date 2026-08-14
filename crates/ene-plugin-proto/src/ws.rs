//! WebSocket broker passenger wire types (protocol v8).
//!
//! A plugin opens a WebSocket session through the host's `WebSocket`
//! passenger: the host validates SSRF and origin approvals, injects the
//! credential by key name, and relays frames. The session is full-duplex —
//! the plugin sends frames with [`WebSocketRequest::SendText`] /
//! [`WebSocketRequest::SendBinary`] and receives pushed
//! [`WebSocketResponse`] frames on the same channel.

use serde::{Deserialize, Serialize};

/// Requests a plugin sends on a WebSocket passenger session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebSocketRequest {
    /// Opens a WebSocket connection. Must be the first frame of a session.
    Open {
        /// Absolute `ws://` or `wss://` URL.
        url: String,
        /// Extra headers (authorization-like headers are stripped unless
        /// the host injects a credential).
        headers: Vec<(String, String)>,
        /// Name of a host-owned credential to inject as
        /// `Authorization: Bearer <value>` at connection time.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credential: Option<String>,
    },
    /// Sends a text frame to the peer.
    SendText {
        /// UTF-8 frame payload.
        data: String,
    },
    /// Sends a binary frame to the peer.
    SendBinary {
        /// Frame payload.
        data: Vec<u8>,
    },
    /// Starts the close handshake with the given status code and reason.
    Close {
        /// WebSocket close code (e.g. 1000).
        code: u16,
        /// Close reason (optional, UTF-8).
        reason: String,
    },
}

/// Frames the host pushes to a plugin WebSocket session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebSocketResponse {
    /// The connection opened. First frame of a successful session.
    OpenOk {
        /// Final URL after any host-side validation.
        final_url: String,
    },
    /// One text message from the peer.
    MessageText {
        /// UTF-8 payload.
        data: String,
    },
    /// One binary message from the peer.
    MessageBinary {
        /// Payload.
        data: Vec<u8>,
    },
    /// The connection closed (peer-initiated, host-initiated, or error).
    Closed {
        /// Close code (`1006` for abnormal closure).
        code: u16,
        /// Close reason.
        reason: String,
    },
    /// A session error; the session is terminated after this frame.
    Error {
        /// HTTP status when the handshake failed on the wire, else `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<u16>,
        /// Human-readable detail.
        message: String,
    },
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit tests use expect")]
mod tests {
    use super::*;

    #[test]
    fn websocket_frames_round_trip() {
        let requests = vec![
            WebSocketRequest::Open {
                url: "wss://example.com/chat".to_string(),
                headers: vec![("origin".to_string(), "app://x".to_string())],
                credential: Some("api_key".to_string()),
            },
            WebSocketRequest::SendText {
                data: "hello".to_string(),
            },
            WebSocketRequest::SendBinary {
                data: vec![0, 1, 2],
            },
            WebSocketRequest::Close {
                code: 1000,
                reason: "done".to_string(),
            },
        ];
        for request in requests {
            let json = serde_json::to_value(&request).expect("serialize");
            let back: WebSocketRequest = serde_json::from_value(json).expect("deserialize");
            assert_eq!(request, back);
        }

        let responses = vec![
            WebSocketResponse::OpenOk {
                final_url: "wss://example.com/chat".to_string(),
            },
            WebSocketResponse::MessageText {
                data: "hi".to_string(),
            },
            WebSocketResponse::MessageBinary { data: vec![9, 8] },
            WebSocketResponse::Closed {
                code: 1000,
                reason: "bye".to_string(),
            },
            WebSocketResponse::Error {
                status: Some(403),
                message: "denied".to_string(),
            },
        ];
        for response in responses {
            let json = serde_json::to_value(&response).expect("serialize");
            let back: WebSocketResponse = serde_json::from_value(json).expect("deserialize");
            assert_eq!(response, back);
        }
    }
}
