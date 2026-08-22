//! Bundled `tool.fs` plugin. Shell execution lives in `tool.exec` (D-24).

#![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests fail fast"))]

mod logic;

// Keep host-only `with_workspace` live in this binary so shared logic.rs stays
// clean under -D warnings without #[allow] (forbidden by clippy::allow_attributes).
#[expect(dead_code, reason = "link stub for host-only API shared with ene-registry")]
fn _link_host_workspace_api() {
    let _ = logic::with_workspace(std::path::Path::new("."), || Ok::<(), String>(()));
}

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
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
