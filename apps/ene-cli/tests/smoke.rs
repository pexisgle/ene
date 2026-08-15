//! No network is touched: chat/tool commands are deliberately not
//! exercised, and the plugin host is disabled so no plugin process spawns.

#![expect(
    clippy::expect_used,
    reason = "smoke tests use expect for concise assertions"
)]

use std::process::Command;

fn ene_bin() -> &'static str {
    option_env!("CARGO_BIN_EXE_ene-cli")
        .or(option_env!("CARGO_BIN_EXE_ene_cli"))
        .expect("cargo sets CARGO_BIN_EXE_* for integration tests")
}

#[test]
fn help_and_version_exit_zero_without_config() {
    let help = Command::new(ene_bin())
        .arg("--help")
        .output()
        .expect("spawn ene --help");
    assert!(help.status.success(), "help must exit 0");
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(help_text.contains("Usage"), "help must print usage");

    let version = Command::new(ene_bin())
        .arg("--version")
        .output()
        .expect("spawn ene --version");
    assert!(version.status.success(), "version must exit 0");
    assert!(String::from_utf8_lossy(&version.stdout).contains("ene"));
}

/// The config is loaded from a temp copy so the config-version migration
/// never rewrites the repo's `settings.json`.
#[test]
fn characters_list_emits_valid_json_without_providers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = dir.path().join("settings.json");
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/settings.json"),
        &settings,
    )
    .expect("copy repo settings into temp dir");
    let output = Command::new(ene_bin())
        .args([
            "--config",
            settings.to_str().expect("utf8 temp path"),
            "characters",
            "list",
            "--json",
        ])
        .env("ENE_PLUGINS__ENABLED", "false")
        .env("ENE_STORE__ENABLED", "false")
        .output()
        .expect("spawn ene characters list");
    assert!(
        output.status.success(),
        "characters list must exit 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout must be valid JSON");
    assert!(parsed.is_array(), "characters list must emit a JSON array");
}

#[test]
fn missing_config_path_exits_nonzero_with_error() {
    let output = Command::new(ene_bin())
        .args([
            "--config",
            "/nonexistent/ene-settings.json",
            "session",
            "list",
        ])
        .env("ENE_PLUGINS__ENABLED", "false")
        .output()
        .expect("spawn ene with missing config");
    assert!(!output.status.success(), "missing config must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.trim().is_empty(),
        "failure must produce a user-facing error on stderr"
    );
}
