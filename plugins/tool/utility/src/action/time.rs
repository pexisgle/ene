use ene_plugin::prelude::*;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "utility",
    name = "get_current_time",
    summary = "Get the current date and time on the user's system.",
    description = "Get the current date and time on the user's system.",
    category = "Utility",
    keywords_primary = "time, date, now, today"
)]
pub struct GetCurrentTimeAction {}

impl GetCurrentTimeAction {
    async fn run(&self) -> Result<String, ToolError> {
        let now = chrono::Local::now();
        Ok(now.format("%Y-%m-%d %H:%M:%S").to_string())
    }
}
