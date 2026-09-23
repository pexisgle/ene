pub mod paths;
pub mod typed;

pub use paths::{default_data_dir, resolve_data_dir};
pub use typed::{Config, ConfigError};
