use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::context::AppContext;
use crate::style;
use async_trait::async_trait;

pub struct ToolCommand;

fn truncate_desc(desc: &str, max_chars: usize) -> String {
    if desc.chars().count() > max_chars {
        let truncated: String = desc.chars().take(max_chars - 3).collect();
        format!("{truncated}...")
    } else {
        desc.to_string()
    }
}

#[async_trait]
impl CliCommand for ToolCommand {
    fn name(&self) -> &'static str {
        "/tool"
    }

    fn description(&self) -> &'static str {
        "Manage and call tools"
    }

    fn usage(&self) -> &'static str {
        "/tool <list|search|help|call>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        let subparts: Vec<&str> = arg.splitn(3, ' ').collect();
        match subparts.first().copied() {
            Some("list") => match ctx.handle.tools().list_tools().await {
                Ok(tools) => {
                    if tools.is_empty() {
                        println!("No tools registered.");
                    } else {
                        println!("{}", style::success("Available tools:"));
                        for tool in tools {
                            let desc = truncate_desc(&tool.description, 60);
                            println!("  - {}: {}", style::header(tool.name.as_str()), desc);
                        }
                    }
                    Ok(CommandOutcome::Continue)
                }
                Err(e) => Err(CliError::ExecutionFailed(format!(
                    "Failed to list tools: {e}"
                ))),
            },
            Some("search") => {
                let query = arg.strip_prefix("search").unwrap_or(arg).trim();
                if query.is_empty() {
                    Err(CliError::UsageError {
                        usage: "Usage: /tool search <query>".to_string(),
                    })
                } else {
                    match ctx.handle.tools().search_tools(query.to_string()).await {
                        Ok(tools) => {
                            if tools.is_empty() {
                                println!("No matching tools found.");
                            } else {
                                println!("{}", style::success("Matching tools:"));
                                for tool in tools {
                                    let desc = truncate_desc(&tool.description, 60);
                                    println!("  - {}: {}", style::header(tool.name.as_str()), desc);
                                }
                            }
                            Ok(CommandOutcome::Continue)
                        }
                        Err(e) => Err(CliError::ExecutionFailed(format!(
                            "Failed to search tools: {e}"
                        ))),
                    }
                }
            }
            Some("help") => {
                if subparts.len() >= 2 {
                    let name = subparts[1];
                    match ctx.handle.tools().list_tools().await {
                        Ok(tools) => {
                            if let Some(tool) = tools.iter().find(|t| t.name.as_str() == name) {
                                println!(
                                    "{}",
                                    style::success(format!("Tool: {}", tool.name.as_str()))
                                );
                                println!("Description: {}", tool.description);
                                println!("Parameters Schema:");
                                println!(
                                    "{}",
                                    serde_json::to_string_pretty(&tool.parameters)
                                        .unwrap_or_default()
                                );
                                Ok(CommandOutcome::Continue)
                            } else {
                                Err(CliError::ExecutionFailed(format!("Tool not found: {name}")))
                            }
                        }
                        Err(e) => Err(CliError::ExecutionFailed(format!(
                            "Failed to retrieve tools: {e}"
                        ))),
                    }
                } else {
                    Err(CliError::UsageError {
                        usage: "Usage: /tool help <tool_name>".to_string(),
                    })
                }
            }
            Some("call") => {
                if subparts.len() >= 3 {
                    let name = subparts[1];
                    let arguments = subparts[2];
                    println!("Calling tool {name} with arguments: {arguments}");
                    match ctx
                        .handle
                        .tools()
                        .call_tool(name.to_string(), arguments.to_string())
                        .await
                    {
                        Ok(res) => {
                            println!("{}", style::success("Tool execution result:"));
                            println!("{res}");
                            Ok(CommandOutcome::Continue)
                        }
                        Err(e) => Err(CliError::ExecutionFailed(format!(
                            "Tool execution failed: {e}"
                        ))),
                    }
                } else if subparts.len() == 2 {
                    let name = subparts[1];
                    println!("Calling tool {name} with empty arguments");
                    match ctx
                        .handle
                        .tools()
                        .call_tool(name.to_string(), "{}".to_string())
                        .await
                    {
                        Ok(res) => {
                            println!("{}", style::success("Tool execution result:"));
                            println!("{res}");
                            Ok(CommandOutcome::Continue)
                        }
                        Err(e) => Err(CliError::ExecutionFailed(format!(
                            "Tool execution failed: {e}"
                        ))),
                    }
                } else {
                    Err(CliError::UsageError {
                        usage: "Usage: /tool call <name> <json>".to_string(),
                    })
                }
            }
            _ => Err(CliError::UsageError {
                usage: self.usage().to_string(),
            }),
        }
    }
}
