//! Wire protocol version 1.
//!
//! Module layout follows the design's remote-capable selection: routing
//! envelope, opaque references, handshake shells, text round trip,
//! presence fact, and setup management inlet. Everything here is serde
//! data; authority, validation, and domain mapping live Host-side.

pub mod envelope;
pub mod handshake;
pub mod management;
pub mod payload;
pub mod presence;
pub mod refs;
pub mod round;
