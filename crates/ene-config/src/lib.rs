//! Typed process configuration: values, OS paths, and JSON schema.
//!
//! Holds no domain state, no secrets, and no runtime judgments.
//!
//! The crate owns three pieces and nothing else:
//!
//! * [`Config`] ([`typed`]): the minimal `M1` configuration value with
//!   layered loading and validation.
//! * OS data directory resolution ([`paths`]): explicit override handling on
//!   top of the OS default, without side effects.
//! * JSON Schema generation ([`schema`]): the machine-readable shape of
//!   [`Config`] for tooling, without file output.
//!
//! This crate has no dependencies on other `ene` crates.

pub mod paths;
pub mod schema;
pub mod typed;

pub use paths::{default_data_dir, resolve_data_dir};
pub use schema::{config_schema, config_schema_json};
pub use typed::{Config, ConfigError};
