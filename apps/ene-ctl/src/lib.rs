//! `ene-ctl` programmatic surface: client transport, command builders,
//! and the shared CLI failure type.
//!
//! The binary (`main.rs`) stays a thin argument-and-stdio shell over these
//! modules so integration tests and tools can drive the same handshake,
//! session, and rendering logic without spawning a process.

#![cfg_attr(
    test,
    allow(
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::panic,
        reason = "test fixtures may unwrap values whose failure would be a fixture bug"
    )
)]

pub mod client;
pub mod cmds;
pub mod device;
pub mod errors;
