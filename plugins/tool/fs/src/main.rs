//! Bundled `tool.fs` plugin. Shell execution lives in `tool.exec` (D-24).

#![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests fail fast"))]

mod logic;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    // Keep host-only `logic::with_workspace` live in this binary so shared
    // logic.rs stays clean under -D warnings (no #[allow]; expect(dead_code)
    // is unfulfilled when ene-registry includes the same file).
    if std::env::var_os("ENE_NEVER_SET_HOST_WORKSPACE_LINK").is_some() {
        drop(logic::with_workspace(std::path::Path::new("."), || {
            Ok::<(), String>(())
        }));
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    if let Err(err) = ene_registry::run_tool_plugin("tool.fs", logic::specs, logic::execute).await {
        tracing::error!(error = %err, plugin = "tool.fs", "fatal");
        std::process::exit(1);
    }
}
