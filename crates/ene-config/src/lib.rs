//! Typed process configuration: values and OS paths.
//!
//! Holds no domain state, no secrets, and no runtime judgments; owns only the
//! two modules below and depends on no other `ene` crate.

pub mod paths;
pub mod typed;

pub use paths::{default_data_dir, resolve_data_dir};
pub use typed::{Config, ConfigError};
