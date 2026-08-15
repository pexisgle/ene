//! Shared payload codec for the framed IPC protocols in this crate.
//!
//! Both the plugin IPC (v6) and the legacy tool IPC (v2) frame messages as a
//! 4-byte little-endian length prefix followed by a payload; this module owns
//! the payload encoding so the two framing layers cannot drift apart.

use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireFormat {
    /// UTF-8 JSON payload.
    Json,
    /// `MessagePack` payload (`rmp-serde`, map-encoded structs).
    MsgPack,
}

impl WireFormat {
    /// Minimum plugin IPC protocol version whose non-handshake frames use
    /// `MessagePack`. The handshake exchange stays JSON at every version
    /// because the host must parse the ack (which carries the negotiated
    /// version) before it can know the peer's format; versions below this
    /// constant keep the original JSON framing so N-1 peers stay
    /// byte-compatible.
    pub const MSGPACK_MIN_PROTOCOL_VERSION: u32 = 6;

    pub const fn for_version(version: u32) -> Self {
        if version >= Self::MSGPACK_MIN_PROTOCOL_VERSION {
            Self::MsgPack
        } else {
            Self::Json
        }
    }

    /// Encodes `value` into a payload in this format.
    ///
    /// Structs are map-encoded (`MessagePack` `to_vec_named`), never
    /// array-encoded, so `#[serde(default)]` fields stay forward-compatible
    /// exactly as they are on the JSON wire.
    pub(crate) fn encode<T: Serialize>(self, value: &T) -> Result<Vec<u8>, WireError> {
        match self {
            Self::Json => serde_json::to_vec(value).map_err(WireError::JsonEncode),
            Self::MsgPack => rmp_serde::to_vec_named(value).map_err(WireError::MsgPackEncode),
        }
    }

    pub(crate) fn decode<T: DeserializeOwned>(self, bytes: &[u8]) -> Result<T, WireError> {
        match self {
            Self::Json => serde_json::from_slice(bytes).map_err(WireError::JsonDecode),
            Self::MsgPack => rmp_serde::from_slice(bytes).map_err(WireError::MsgPackDecode),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum WireError {
    #[error("JSON serialization failed: {0}")]
    JsonEncode(serde_json::Error),
    #[error("JSON deserialization failed: {0}")]
    JsonDecode(serde_json::Error),
    #[error("MessagePack serialization failed: {0}")]
    MsgPackEncode(rmp_serde::encode::Error),
    #[error("MessagePack deserialization failed: {0}")]
    MsgPackDecode(rmp_serde::decode::Error),
}
