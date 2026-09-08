//! Host composition root (`Stage 2` entrypoint).
//!
//! `ene-core` is the Host composition root: wiring and lifecycle only. It
//! performs no semantic judgment and owns no domain state beyond the
//! [`ene_core::serve::HostHandle`] it builds in `serve` mode; the domain
//! pipelines live in the library modules, and this binary holds argument
//! parsing plus process setup.
//!
//! With no subcommand the entrypoint keeps the `Stage 1` behavior: parse
//! arguments, [`Config::load`] (validation included), and
//! [`ene_config::resolve_data_dir`] proof without effects. With the `serve`
//! subcommand it resolves the data directory and blocks on
//! [`ene_core::serve::serve`]: the Unix socket listener serving the full
//! orchestration pipeline.

use std::path::{Path, PathBuf};

use ene_config::Config;
use ene_core::serve::{self, CoreError};

/// Command-line failure for the Host entrypoint.
///
/// [`CliError::Usage`] covers argument misuse; [`CliError::Config`] carries a
/// [`ene_config::typed::ConfigError`] from [`Config::load`] unchanged, and
/// [`CliError::Serve`] carries a [`CoreError`] from `serve` mode unchanged.
#[derive(Debug, thiserror::Error)]
enum CliError {
    /// Command-line usage was violated.
    ///
    /// The display always contains the usage line
    /// `usage: ene-core [--config PATH] [serve]` followed by the detail, so
    /// callers can assert on the usage line alone. The bracketed prefix keeps
    /// the `Stage 1` usage line as a substring.
    #[error("usage: ene-core [--config PATH] [serve]: {0}")]
    Usage(String),
    /// Layered configuration loading or validation failed.
    #[error(transparent)]
    Config(#[from] ene_config::typed::ConfigError),
    /// `serve` mode failed.
    #[error(transparent)]
    Serve(#[from] CoreError),
}

/// Parses Host command-line arguments, excluding the program name.
///
/// Accepts exactly one form: `--config PATH`, which selects an explicit JSON
/// configuration file. With no arguments there is no override and the result
/// is [`None`]. A repeated `--config` keeps the last value; later flags
/// override earlier ones, matching the usual override convention. The value
/// following `--config` is consumed verbatim, even when it starts with `--`.
/// A missing value after `--config` and any unknown argument (including
/// `--help` and `--version`) are [`CliError::Usage`] failures whose display
/// contains the usage line.
///
/// The function is pure: it inspects only `args` and never touches the
/// process environment, the filesystem, or `stdout`.
///
/// # Errors
///
/// Returns [`CliError::Usage`] when `--config` has no following value or when
/// any other argument is present.
fn parse_args(args: &[String]) -> Result<Option<PathBuf>, CliError> {
    let mut config: Option<PathBuf> = None;
    let mut pending = args.iter();
    while let Some(arg) = pending.next() {
        if arg.as_str() == "--config" {
            let Some(value) = pending.next() else {
                return Err(CliError::Usage("missing value for --config".to_string()));
            };
            config = Some(PathBuf::from(value));
        } else {
            return Err(CliError::Usage(format!("unknown argument: {arg}")));
        }
    }
    Ok(config)
}

/// Splits the `serve` subcommand off the argument list, order-independent.
///
/// Scans `args` for exactly one `serve` token and returns it separately from
/// the remaining arguments (which [`parse_args`] then parses for `--config`).
/// Both `ene-core serve --config PATH` and `ene-core --config PATH serve`
/// work; a repeated `serve` is a [`CliError::Usage`] failure.
///
/// The function is pure: it inspects only `args` and never touches the
/// process environment, the filesystem, or `stdout`.
///
/// # Errors
///
/// Returns [`CliError::Usage`] when `serve` appears more than once.
fn extract_serve(args: &[String]) -> Result<(bool, Vec<String>), CliError> {
    let mut serve = false;
    let mut rest = Vec::new();
    for arg in args {
        if arg.as_str() == "serve" {
            if serve {
                return Err(CliError::Usage("duplicate subcommand: serve".to_string()));
            }
            serve = true;
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((serve, rest))
}

/// `Stage 2` Host entrypoint: parse arguments, load configuration, then stop
/// or serve.
///
/// Without `serve` this keeps the `Stage 1` behavior: [`parse_args`],
/// [`Config::load`] (which validates), and [`ene_config::resolve_data_dir`]
/// proof with no effects. With `serve` it resolves the data directory (which
/// must exist as a value: an unresolvable directory is a [`CoreError::Store`]
/// failure, since serving without durable state is meaningless) and blocks on
/// [`serve::serve`] under a multi-threaded `Tokio` runtime.
///
/// There is deliberately no `--help` or `--version` handling yet: there is no
/// `stdout` mechanism under the workspace `print_stdout` deny, so they
/// currently report [`CliError::Usage`].
///
/// # Errors
///
/// Returns [`CliError::Usage`] for argument misuse, [`CliError::Config`] when
/// [`Config::load`] fails, and [`CliError::Serve`] when `serve` mode fails.
fn main() -> Result<(), CliError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (serve_mode, rest) = extract_serve(&args)?;
    let path = parse_args(&rest)?;
    let cfg = Config::load(path.as_deref())?;
    if serve_mode {
        let Some(data_dir) = ene_config::resolve_data_dir(&cfg) else {
            return Err(CoreError::Store("no data directory resolved".to_string()).into());
        };
        run_serve(&data_dir)?;
        return Ok(());
    }
    let _data_dir = ene_config::resolve_data_dir(&cfg);
    Ok(())
}

/// Blocks the calling thread on [`serve::serve`] for `data_dir`.
///
/// Builds the multi-threaded `Tokio` runtime the listener and the store tasks
/// run on. A runtime that cannot be built is a [`CoreError::Store`] failure:
/// the runtime is the async substrate of the store-backed Host, and no
/// narrower variant names it.
///
/// # Errors
///
/// Returns [`CoreError::Store`] when the runtime cannot be built and the
/// [`serve::serve`] error otherwise.
fn run_serve(data_dir: &Path) -> Result<(), CoreError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| CoreError::Store("tokio runtime unavailable".to_string()))?;
    runtime.block_on(serve::serve(data_dir))
}

#[cfg(test)]
mod tests {
    use super::{extract_serve, parse_args};
    use std::path::PathBuf;

    #[test]
    fn no_args_yields_no_override() {
        let args: Vec<String> = Vec::new();
        let path = parse_args(&args).expect("no args must succeed");
        assert!(path.is_none(), "no args must yield no override");
    }

    #[test]
    fn config_flag_captures_its_value() {
        let args = [String::from("--config"), String::from("/tmp/ene.json")];
        let path = parse_args(&args).expect("--config with a value must succeed");
        assert!(
            path == Some(PathBuf::from("/tmp/ene.json")),
            "the --config value must become the override"
        );
    }

    #[test]
    fn missing_config_value_is_a_usage_error() {
        let args = [String::from("--config")];
        let error = parse_args(&args).expect_err("a missing --config value must fail");
        let rendered = format!("{error}");
        assert!(
            rendered.contains("usage: ene-core [--config PATH]"),
            "a missing value must render the usage line: {rendered}"
        );
    }

    #[test]
    fn unknown_argument_is_a_usage_error() {
        let args = [String::from("--verbose")];
        let error = parse_args(&args).expect_err("an unknown argument must fail");
        let rendered = format!("{error}");
        assert!(
            rendered.contains("usage: ene-core [--config PATH]"),
            "an unknown argument must render the usage line: {rendered}"
        );
    }

    #[test]
    fn repeated_config_keeps_the_last_value() {
        let args = [
            String::from("--config"),
            String::from("/tmp/first.json"),
            String::from("--config"),
            String::from("/tmp/second.json"),
        ];
        let path = parse_args(&args).expect("a repeated --config must succeed");
        assert!(
            path == Some(PathBuf::from("/tmp/second.json")),
            "a repeated --config must keep the last value"
        );
    }

    #[test]
    fn bare_args_select_no_subcommand() {
        let args = [String::from("--config"), String::from("/tmp/ene.json")];
        let split = extract_serve(&args);
        assert!(split.is_ok(), "args without serve must split");
        let Some((serve, rest)) = split.ok() else {
            return;
        };
        assert!(!serve, "no serve token means no subcommand");
        assert_eq!(rest, args, "the rest must pass through untouched");
    }

    #[test]
    fn serve_token_splits_off_in_any_position() {
        for args in [
            vec![String::from("serve")],
            vec![
                String::from("serve"),
                String::from("--config"),
                String::from("/tmp/e.json"),
            ],
            vec![
                String::from("--config"),
                String::from("/tmp/e.json"),
                String::from("serve"),
            ],
        ] {
            let split = extract_serve(&args);
            assert!(split.is_ok(), "a single serve must split: {args:?}");
            let Some((serve, rest)) = split.ok() else {
                return;
            };
            assert!(serve, "the serve token must select the subcommand");
            assert!(
                !rest.iter().any(|arg| arg == "serve"),
                "the rest must not keep serve: {rest:?}"
            );
        }
    }

    #[test]
    fn repeated_serve_is_a_usage_error() {
        let args = [String::from("serve"), String::from("serve")];
        let split = extract_serve(&args);
        assert!(split.is_err(), "a repeated serve must fail");
        let Some(error) = split.err() else {
            return;
        };
        let rendered = format!("{error}");
        assert!(
            rendered.contains("usage: ene-core [--config PATH]"),
            "a repeated serve must render the usage line: {rendered}"
        );
    }
}
