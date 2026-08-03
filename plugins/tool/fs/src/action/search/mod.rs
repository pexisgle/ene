pub mod search_glob;
pub mod search_grep;
pub mod search_regex;

pub use search_grep::GrepOptions;

use crate::utils::sandbox::SandboxConfig;
use ene_plugin_proto::ToolError;

pub const MAX_RESULTS: usize = 100;

#[expect(
    clippy::unused_async,
    reason = "tool IPC handlers are async for uniform provider dispatch"
)]
pub async fn glob_search(
    pattern: &str,
    path: Option<&str>,
    sandbox: &SandboxConfig,
) -> Result<String, ToolError> {
    search_glob::glob_search(pattern, path, sandbox)
}

pub async fn grep_search(
    pattern: &str,
    path: Option<&str>,
    include: Option<&str>,
    options: &GrepOptions,
    sandbox: &SandboxConfig,
) -> Result<String, ToolError> {
    search_grep::grep_search(pattern, path, include, options, sandbox).await
}
