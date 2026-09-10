//! `ene-ctl` CLI client entrypoint (Stage 1).
//!
//! Thin client skeleton: parses `--config PATH`, loads [`Config`], and
//! resolves the effective data directory. The client holds no canonical state
//! and establishes no local authority of its own; round identity stays
//! Host-issued.

use std::path::PathBuf;

use ene_config::paths::resolve_data_dir;
use ene_config::typed::{Config, ConfigError};

/// Usage line reported with every [`CliError::Usage`].
const USAGE: &str = "usage: ene-ctl [--config PATH]";

/// CLI failure: bad arguments or configuration load failure.
#[derive(Debug, thiserror::Error)]
enum CliError {
    /// Command-line usage failure; the message always ends with [`USAGE`].
    #[error("{0}")]
    Usage(String),
    /// Layered configuration loading or validation failed.
    #[error(transparent)]
    Config(#[from] ConfigError),
}

/// Parses `--config PATH` from `args` (excluding the program name).
///
/// Returns [`None`] when no arguments are given. A missing `--config` value
/// or an unknown argument is a [`CliError::Usage`] whose message ends with
/// [`USAGE`]. `--help` and `--version` are deferred to a later stage, so they
/// are reported as unknown arguments for now.
fn parse_args(args: &[String]) -> Result<Option<PathBuf>, CliError> {
    let mut config: Option<PathBuf> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--config" {
            let Some(value) = iter.next() else {
                return Err(CliError::Usage(format!(
                    "missing value for --config\n{USAGE}"
                )));
            };
            config = Some(PathBuf::from(value));
        } else {
            return Err(CliError::Usage(format!("unknown argument: {arg}\n{USAGE}")));
        }
    }
    Ok(config)
}

/// Client entrypoint: load configuration and resolve the data directory.
///
/// Synchronous on purpose; no async runtime is needed until a later stage
/// constructs and sends DTOs via `ene-api` only (there are no commands yet).
/// Resolving the data directory establishes no local authority and attempts
/// no connection; it only determines which directory a future stage would
/// stage from, while round identity stays Host-issued.
fn main() -> Result<(), CliError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config_path = parse_args(&args)?;
    let cfg = Config::load(config_path.as_deref())?;
    let _data_dir = resolve_data_dir(&cfg);
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Unit tests for [`parse_args`](super::parse_args).
    //!
    //! The parser is pure over its input slice, so every case runs without
    //! touching the process environment.

    use std::path::Path;

    use super::{USAGE, parse_args};

    /// Builds owned arguments from plain words.
    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    /// No arguments select no config file.
    #[test]
    fn no_args_selects_no_config_file() {
        assert!(
            matches!(parse_args(&args(&[])), Ok(None)),
            "no arguments must select no config file"
        );
    }

    /// `--config PATH` selects that file.
    #[test]
    fn config_flag_selects_a_file() {
        let path = parse_args(&args(&["--config", "/tmp/ene.json"]))
            .expect("--config with a value must succeed")
            .expect("--config must select a file");
        assert!(
            path.as_path() == Path::new("/tmp/ene.json"),
            "--config must select the given file"
        );
    }

    /// A repeated `--config` keeps the last value.
    #[test]
    fn repeated_config_flag_keeps_the_last_value() {
        let path = parse_args(&args(&[
            "--config",
            "/tmp/a.json",
            "--config",
            "/tmp/b.json",
        ]))
        .expect("a repeated --config must succeed")
        .expect("a repeated --config must select a file");
        assert!(
            path.as_path() == Path::new("/tmp/b.json"),
            "a repeated --config must keep the last value"
        );
    }

    /// A missing `--config` value reports usage.
    #[test]
    fn missing_config_value_reports_usage() {
        let super::CliError::Usage(message) = parse_args(&args(&["--config"]))
            .expect_err("a missing --config value must be a usage error")
        else {
            panic!("a missing --config value must be a usage error");
        };
        assert!(
            message.contains(USAGE),
            "a missing --config value must report the usage line: {message:?}"
        );
    }

    /// An unknown argument reports usage.
    #[test]
    fn unknown_argument_reports_usage() {
        let super::CliError::Usage(message) = parse_args(&args(&["--unknown"]))
            .expect_err("an unknown argument must be a usage error")
        else {
            panic!("an unknown argument must be a usage error");
        };
        assert!(
            message.contains(USAGE),
            "an unknown argument must report the usage line: {message:?}"
        );
    }

    /// A stray positional argument reports usage.
    #[test]
    fn positional_argument_reports_usage() {
        let super::CliError::Usage(message) =
            parse_args(&args(&["extra"])).expect_err("a positional argument must be a usage error")
        else {
            panic!("a positional argument must be a usage error");
        };
        assert!(
            message.contains(USAGE),
            "a positional argument must report the usage line: {message:?}"
        );
    }

    /// Deferred flags such as `--help` report usage for now.
    #[test]
    fn help_flag_reports_usage_while_deferred() {
        let super::CliError::Usage(message) = parse_args(&args(&["--help"]))
            .expect_err("--help must be a usage error while deferred")
        else {
            panic!("--help must be a usage error while deferred");
        };
        assert!(
            message.contains(USAGE),
            "--help must report the usage line while deferred: {message:?}"
        );
    }
}
