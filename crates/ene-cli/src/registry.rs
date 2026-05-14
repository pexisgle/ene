use std::sync::Arc;
use ene_ai_core::{
    config::AiSettings,
    mcp_client::McpToolRegistry,
    sandbox::SandboxConfig,
    tool::ToolRegistry,
    tool_factory::ToolRegistryBuilder,
};

pub async fn build(settings: &AiSettings) -> Arc<dyn ToolRegistry> {
    if settings.mcp_servers.is_empty() {
        let mut builder = ToolRegistryBuilder::new()
            .with_builtin()
            .with_screenshot(settings.screenshot_scale_percent);

        if settings.sandbox.enabled {
            builder = builder.with_sandbox(build_sandbox_config(settings));
        }

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
        Box::new(ene_ai_core::builtin_tools::BuiltinToolRegistry::new()),
        Box::new(
            ene_ai_core::screenshot_tool::ScreenshotToolRegistry::new(
                settings.screenshot_scale_percent,
            ),
        ),
    ];
    if settings.sandbox.enabled {
        registries.push(Box::new(
            ene_ai_core::tools::OpencodeToolRegistry::new(build_sandbox_config(settings)),
        ));
    }
    registries.push(Box::new(mcp));
    Arc::new(ene_ai_core::composite_registry::CompositeToolRegistry::new(registries))
}

fn build_sandbox_config(settings: &AiSettings) -> SandboxConfig {
    let canonicalize_dirs = |dirs: &[String]| -> Vec<std::path::PathBuf> {
        dirs.iter()
            .map(|s| {
                let p = std::path::PathBuf::from(s);
                if p.exists() {
                    std::fs::canonicalize(&p).unwrap_or(p)
                } else {
                    p
                }
            })
            .collect()
    };

    SandboxConfig {
        allowed_directories: canonicalize_dirs(&settings.sandbox.allowed_directories),
        writable_directories: canonicalize_dirs(&settings.sandbox.writable_directories),
        blocked_commands: settings.sandbox.blocked_commands.clone(),
        max_read_bytes: settings.sandbox.max_read_bytes,
        max_write_bytes: settings.sandbox.max_write_bytes,
        shell_timeout_ms: settings.sandbox.shell_timeout_ms,
        max_shell_output_bytes: settings.sandbox.max_shell_output_bytes,
        max_shell_output_lines: settings.sandbox.max_shell_output_lines,
    }
}
