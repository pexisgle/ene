use crate::commands::CliCommand;
use crate::context::AppContext;
use crate::style;
use async_trait::async_trait;

pub struct ToolCommand;

#[async_trait]
impl CliCommand for ToolCommand {
    fn name(&self) -> &'static str {
        "/tool"
    }

    fn description(&self) -> &'static str {
        "Manage and call tools"
    }

    fn usage(&self) -> &'static str {
        "/tool <list|help|call>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), String> {
        let subparts: Vec<&str> = arg.splitn(3, ' ').collect();
        match subparts.first().copied() {
            Some("list") => match ctx.handle.list_tools().await {
                Ok(tools) => {
                    if tools.is_empty() {
                        println!("No tools registered.");
                    } else {
                        println!("{}", style::success("Available tools:"));
                        for tool in tools {
                            let desc = if tool.description.len() > 60 {
                                format!("{}...", &tool.description[..57])
                            } else {
                                tool.description.clone()
                            };
                            println!("  - {}: {}", style::header(tool.name.as_str()), desc);
                        }
                    }
                }
                Err(e) => {
                    println!("{}", style::error(format!("Failed to list tools: {e}")));
                }
            },
            Some("help") => {
                if subparts.len() >= 2 {
                    let name = subparts[1];
                    match ctx.handle.list_tools().await {
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
                            } else {
                                println!("{}", style::error(format!("Tool not found: {name}")));
                            }
                        }
                        Err(e) => {
                            println!("{}", style::error(format!("Failed to retrieve tools: {e}")));
                        }
                    }
                } else {
                    println!("{}", style::warning("Usage: /tool help <tool_name>"));
                }
            }
            Some("call") => {
                if subparts.len() >= 3 {
                    let name = subparts[1];
                    let arguments = subparts[2];
                    println!("Calling tool {name} with arguments: {arguments}");
                    match ctx
                        .handle
                        .call_tool(name.to_string(), arguments.to_string())
                        .await
                    {
                        Ok(res) => {
                            println!("{}", style::success("Tool execution result:"));
                            println!("{res}");
                        }
                        Err(e) => {
                            println!("{}", style::error(format!("Tool execution failed: {e}")));
                        }
                    }
                } else if subparts.len() == 2 {
                    let name = subparts[1];
                    println!("Calling tool {name} with empty arguments");
                    match ctx
                        .handle
                        .call_tool(name.to_string(), "{}".to_string())
                        .await
                    {
                        Ok(res) => {
                            println!("{}", style::success("Tool execution result:"));
                            println!("{res}");
                        }
                        Err(e) => {
                            println!("{}", style::error(format!("Tool execution failed: {e}")));
                        }
                    }
                } else {
                    println!("{}", style::warning("Usage: /tool call <name> <json>"));
                }
            }
            _ => {
                println!(
                    "{}",
                    style::warning(
                        "Usage: /tool list | /tool help <name> | /tool call <name> <json>"
                    )
                );
            }
        }
        Ok(())
    }
}
