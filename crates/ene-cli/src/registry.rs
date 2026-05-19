use ene_ai_core::{
    config::AiSettings, mcp_client::McpToolRegistry,
    tool_factory::ToolRegistryBuilder, tools::{CompositeToolRegistry, ToolRegistry},
};
use std::sync::Arc;

pub async fn build(settings: &AiSettings) -> Arc<dyn ToolRegistry> {
    let mut builder = ToolRegistryBuilder::new()
        .with_builtin()
        .with_screenshot(settings.screenshot_scale_percent);

    if settings.sandbox.enabled {
        builder = builder.with_sandbox(settings.sandbox.to_sandbox_config());
    }

    if settings.mcp_servers.is_empty() {
        return builder.build();
    }

    let mcp = McpToolRegistry::new();
    for server in &settings.mcp_servers {
        if !server.enabled {
            continue;
        }

        match &server.transport {
            ene_ai_core::config::McpTransport::Stdio { command, args } => {
                let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                if let Err(err) = mcp.connect_stdio(&server.name, command, &args_ref).await {
                    eprintln!(
                        "Warning: MCP server '{}' failed to connect: {}",
                        server.name, err
                    );
                }
            }
            ene_ai_core::config::McpTransport::Http { url } => {
                eprintln!(
                    "Warning: MCP HTTP transport not supported yet for '{}': {}",
                    server.name, url
                );
            }
        }
    }

    let mut registries: Vec<Box<dyn ToolRegistry>> = vec![
        Box::new(ene_ai_core::BuiltinToolRegistry::new()),
        Box::new(ene_ai_core::ScreenshotToolRegistry::new(
            settings.screenshot_scale_percent,
        )),
    ];
    if settings.sandbox.enabled {
        registries.push(Box::new(ene_ai_core::tools::OpencodeToolRegistry::new(
            settings.sandbox.to_sandbox_config(),
        )));
    }
    registries.push(Box::new(mcp));
    Arc::new(CompositeToolRegistry::new(registries))
}
