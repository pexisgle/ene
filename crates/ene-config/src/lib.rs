//! Typed process configuration: values, OS paths, and JSON schema.
//!
//! Holds no domain state, no secrets, and no runtime judgments; owns only the
//! three modules below and depends on no other `ene` crate.

pub mod paths;
pub mod schema;
pub mod typed;

pub use paths::{default_data_dir, resolve_data_dir};
pub use schema::{config_schema, config_schema_json};
pub use typed::{Config, ConfigError};
