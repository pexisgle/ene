//! Host composition root (`Stage 1` entrypoint).
//!
//! `ene-core` is the Host composition root: wiring and lifecycle only. It
//! performs no semantic judgment, owns no domain state, and depends on no
//! `ene-vrm` or Client adapters; [`Config`] plus OS path resolution is the
//! whole `Stage 1` surface.
//!
//! `Stage 1` scope is argument parsing, [`Config::load`] (validation
//! included), and [`ene_config::resolve_data_dir`] proof. There is
//! deliberately no serve loop, no listener, no database, no provider, no
//! presence, and no management surface yet; those arrive in `Stage 2` and
//! later.

use std::path::PathBuf;

use ene_config::Config;

/// Command-line failure for the `Stage 1` Host entrypoint.
///
/// [`CliError::Usage`] covers argument misuse; [`CliError::Config`] carries a
/// [`ene_config::typed::ConfigError`] from [`Config::load`] unchanged.
#[derive(Debug, thiserror::Error)]
enum CliError {
    /// Command-line usage was violated.
    ///
    /// The display always contains the usage line
    /// `usage: ene-core [--config PATH]` followed by the detail, so callers
    /// can assert on the usage line alone.
    #[error("usage: ene-core [--config PATH]: {0}")]
    Usage(String),
    /// Layered configuration loading or validation failed.
    #[error(transparent)]
    Config(#[from] ene_config::typed::ConfigError),
}

/// Parses Host command-line arguments, excluding the program name.
///
/// Accepts exactly one form: `--config PATH`, which selects an explicit JSON
/// configuration file. With no arguments there is no override and the result
/// is [`None`]. A repeated `--config` keeps the last value; later flags
/// override earlier ones, matching the usual override convention. The value
/// following `--config` is consumed verbatim, even when it starts with `--`.
/// A missing value after `--config` and any unknown argument (including
/// `--help` and `--version`, which Stage 1 does not implement yet) are
/// [`CliError::Usage`] failures whose display contains the usage line.
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

/// `Stage 1` Host entrypoint: parse arguments, load configuration, resolve the
/// data directory, then stop.
///
/// The three steps are [`parse_args`], [`Config::load`] (which validates),
/// and [`ene_config::resolve_data_dir`]. The resolved directory is bound as
/// `_data_dir` with an underscore prefix on purpose: resolution is a pure
/// computation that performs no I/O, creates no directories, and prints
/// nothing, and `Stage 1` allows no effect that would give it meaning (no
/// `mkdir`, no database open, no print). The binding proves the resolution
/// call compiles and runs while deferring every effect to `Stage 2`.
///
/// There is deliberately no serve loop, no listener, no database, no
/// provider, no presence, and no management surface yet (`Stage 2` and
/// later). This is a synchronous `fn main`: there are no I/O boundaries yet,
/// so no `Tokio` runtime. `--help` and `--version` are not implemented in
/// Stage 1, so they currently report [`CliError::Usage`].
///
/// # Errors
///
/// Returns [`CliError::Usage`] for argument misuse and [`CliError::Config`]
/// when [`Config::load`] fails.
fn main() -> Result<(), CliError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = parse_args(&args)?;
    let cfg = Config::load(path.as_deref())?;
    let _data_dir = ene_config::resolve_data_dir(&cfg);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_args;
    use std::path::PathBuf;

    #[test]
    fn no_args_yields_no_override() {
        let args: Vec<String> = Vec::new();
        let parsed = parse_args(&args);
        assert!(parsed.is_ok(), "no args must succeed");
        let Some(path) = parsed.ok() else {
            return;
        };
        assert!(path.is_none(), "no args must yield no override");
    }

    #[test]
    fn config_flag_captures_its_value() {
        let args = [String::from("--config"), String::from("/tmp/ene.json")];
        let parsed = parse_args(&args);
        assert!(parsed.is_ok(), "--config with a value must succeed");
        let Some(path) = parsed.ok() else {
            return;
        };
        assert!(
            path == Some(PathBuf::from("/tmp/ene.json")),
            "the --config value must become the override"
        );
    }

    #[test]
    fn missing_config_value_is_a_usage_error() {
        let args = [String::from("--config")];
        let parsed = parse_args(&args);
        assert!(parsed.is_err(), "a missing --config value must fail");
        let Some(error) = parsed.err() else {
            return;
        };
        let rendered = format!("{error}");
        assert!(
            rendered.contains("usage: ene-core [--config PATH]"),
            "a missing value must render the usage line: {rendered}"
        );
    }

    #[test]
    fn unknown_argument_is_a_usage_error() {
        let args = [String::from("--verbose")];
        let parsed = parse_args(&args);
        assert!(parsed.is_err(), "an unknown argument must fail");
        let Some(error) = parsed.err() else {
            return;
        };
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
        let parsed = parse_args(&args);
        assert!(parsed.is_ok(), "a repeated --config must succeed");
        let Some(path) = parsed.ok() else {
            return;
        };
        assert!(
            path == Some(PathBuf::from("/tmp/second.json")),
            "a repeated --config must keep the last value"
        );
    }
}
