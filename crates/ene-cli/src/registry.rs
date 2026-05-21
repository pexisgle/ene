use ene_ai_core::{
    config::AiSettings,
    mcp_client::McpToolRegistry,
    tool_host_manager::ToolHostManager,
    tools::ToolRegistry,
};
use std::sync::Arc;

pub async fn build(settings: &AiSettings) -> Arc<dyn ToolRegistry> {
    let mut manager = match ToolHostManager::start(settings).await {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[ToolHostManager] Warning: {}", e);
            ToolHostManager::start(&AiSettings {
                tools: ene_ai_core::config::AiToolSettings {
                    enabled: Vec::new(),
                },
                ..settings.clone()
            })
            .await
            .unwrap_or_else(|e| {
                eprintln!("[ToolHostManager] Fatal: {}", e);
                panic!("Failed to start tool host manager");
            })
        }
    };

    if !settings.mcp_servers.is_empty() {
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
        manager.add_registry(Box::new(mcp));
    }

    manager.into_registry()
}