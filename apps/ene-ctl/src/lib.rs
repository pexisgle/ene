//! `ene-ctl` programmatic surface: client transport, command builders,
//! and the shared CLI failure type.
//!
//! The binary (`main.rs`) stays a thin argument-and-stdio shell over these
//! modules so integration tests and tools can drive the same handshake,
//! session, and rendering logic without spawning a process.

pub mod client;
pub mod cmds;
pub mod device;
pub mod errors;
