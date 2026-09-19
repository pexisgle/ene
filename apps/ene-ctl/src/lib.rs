//! `ene-ctl` programmatic surface: the binary (`main.rs`) stays a thin
//! argument-and-stdio shell over [`ene_client`] so integration tests and
//! tools can drive the same handshake, session, and rendering logic without
//! spawning a process. This crate does not speak Host-local control.

pub mod cmds;
pub mod errors;

pub use ene_client as client;
pub use ene_client::device;
pub use ene_client::incarnation;
