use ene_tool_sdk::prelude::*;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "utility",
    name = "get_system_info",
    summary = "Get basic information about the user's system.",
    description = "Get basic information about the user's system.",
    category = "Utility",
    keywords_primary = "system, os, platform, arch, info"
)]
/// Action to get basic system information.
pub struct GetSystemInfoAction {}

impl GetSystemInfoAction {
    async fn run(&self) -> Result<String, ToolError> {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        Ok(format!("OS: {os}, Architecture: {arch}"))
    }
}
