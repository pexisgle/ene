//! # ene-common
//!
//! Shared utilities, error types, and ID newtypes used across the ene workspace.
#![warn(missing_docs)]

/// Common error types for the ene platform.
pub mod error;
/// Type-safe identifiers for domain concepts.
pub mod types;
/// Shared utility functions.
pub mod utils;

/// Re-export commonly used items.
pub use error::CommonError;
pub use types::{CardName, RequestId, SessionId, ToolName};
pub use utils::truncate;
