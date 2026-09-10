//! `ene-ctl` programmatic surface: the binary (`main.rs`) stays a thin
//! argument-and-stdio shell over these modules so integration tests and tools
//! can drive the same handshake, session, and rendering logic without
//! spawning a process.

pub mod client;
pub mod cmds;
pub mod device;
pub mod errors;
