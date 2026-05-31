//! # ene-common
//!
//! Shared string truncation utilities used across the ene workspace.
#![warn(missing_docs)]

/// Smart content truncation (by chars, lines, and tail).
pub mod truncate;

pub use truncate::{Truncate, TruncateResult};
