use std::path::Path;

use ene_fiber::{ProfileRow, discover_plugin_executable};
use ene_kernel::AiSettings;
use ene_work::{McpServer, WorkError, WorkStore};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct McpFile {
    #[serde(default)]
    servers: Vec<McpServer>,
}

pub fn collect_rows(data_dir: &Path, work: &WorkStore, ai: &AiSettings) -> Vec<ProfileRow> {
    let mut rows = harness_rows();
    rows.extend(provider_rows(ai));
    rows.extend(mcp_rows(&load_servers(data_dir, work)));
    rows
}

pub fn load_servers(data_dir: &Path, work: &WorkStore) -> Vec<McpServer> {
    let path = data_dir.join("mcp.json");
    if path.exists() {
        match load_mcp_json(&path) {
            Ok(servers) => {
                if let Err(err) = work.replace_mcp(&servers) {
                    tracing::warn!(error = %err, "mcp store sync failed");
                }
                return servers;
            }
            Err(err) => tracing::warn!(error = %err, "mcp.json unreadable"),
        }
    }
    work.list_mcp().unwrap_or_else(|err| {
        tracing::warn!(error = %err, "mcp store unreadable");
        Vec::new()
    })
}

pub fn save_servers(
    data_dir: &Path,
    work: &WorkStore,
    servers: &[McpServer],
) -> Result<(), WorkError> {
    write_mcp_json(&data_dir.join("mcp.json"), servers)?;
    work.replace_mcp(servers)?;
    Ok(())
}

fn write_mcp_json(path: &Path, servers: &[McpServer]) -> Result<(), WorkError> {
    let body = serde_json::to_string_pretty(&McpFile {
        servers: servers.to_vec(),
    })
    .map_err(|err| WorkError::Codec(err.to_string()))?;
    std::fs::write(path, body)?;
    Ok(())
}

fn load_mcp_json(path: &Path) -> Result<Vec<McpServer>, WorkError> {
    let raw = std::fs::read_to_string(path)?;
    let file: McpFile =
        serde_json::from_str(&raw).map_err(|err| WorkError::Codec(err.to_string()))?;
    Ok(file.servers)
}

fn harness_rows() -> Vec<ProfileRow> {
    [
        "tool.utility",
        "tool.fs",
        "tool.exec",
        "tool.web",
        "tool.app",
    ]
    .into_iter()
    .map(harness_row)
    .collect()
}

fn harness_row(plugin: &str) -> ProfileRow {
    let binary = discover_plugin_executable(plugin);
    let needs_sandbox = matches!(plugin, "tool.fs" | "tool.exec" | "tool.web");
    ProfileRow {
        row_id: plugin.to_owned(),
        plugin: plugin.to_owned(),
        requires: Vec::new(),
        capabilities: Vec::new(),
        sandbox_required: needs_sandbox && binary.is_some(),
        config: serde_json::Value::Null,
    }
}

fn provider_rows(ai: &AiSettings) -> Vec<ProfileRow> {
    let mut seen = std::collections::BTreeSet::new();
    let mut rows = Vec::new();
    for binding in [
        &ai.tasks.chat,
        &ai.tasks.classifier,
        &ai.tasks.embedding,
        &ai.tasks.proactive,
        &ai.tasks.tts,
        &ai.tasks.stt,
    ] {
        if binding.uses_echo() || !binding.plugin.starts_with("provider.") {
            continue;
        }
        if !seen.insert(binding.plugin.clone()) {
            continue;
        }
        if discover_plugin_executable(&binding.plugin).is_none() {
            tracing::warn!(plugin = %binding.plugin, "provider binary missing; skipping");
            continue;
        }
        rows.push(ProfileRow {
            row_id: binding.plugin.clone(),
            plugin: binding.plugin.clone(),
            requires: Vec::new(),
            capabilities: Vec::new(),
            sandbox_required: false,
            config: serde_json::json!({
                "base_url": binding.base_url,
                "model": binding.model,
            }),
        });
    }
    rows
}

fn mcp_rows(servers: &[McpServer]) -> Vec<ProfileRow> {
    if discover_plugin_executable("mcp.bridge").is_none() {
        if servers.iter().any(|server| server.enabled) {
            tracing::warn!("ene-harness-mcp missing; handwritten MCP rows skipped");
        }
        return Vec::new();
    }
    servers
        .iter()
        .filter(|server| server.enabled)
        .filter_map(mcp_row)
        .collect()
}

fn mcp_row(server: &McpServer) -> Option<ProfileRow> {
    if !valid_mcp_id(&server.id) {
        tracing::warn!(id = %server.id, "skipping MCP row with invalid id");
        return None;
    }
    if server.is_http() {
        if server.url.as_deref().unwrap_or("").is_empty() {
            tracing::warn!(id = %server.id, "http MCP row needs url");
            return None;
        }
    } else if server.command.as_deref().unwrap_or("").is_empty() {
        tracing::warn!(id = %server.id, "stdio MCP row needs command");
        return None;
    }
    let plugin = server.plugin_id();
    Some(ProfileRow {
        row_id: plugin.clone(),
        plugin,
        requires: Vec::new(),
        capabilities: Vec::new(),
        sandbox_required: false,
        config: serde_json::json!({
            "server": server.id,
            "transport": if server.is_http() { "http" } else { "stdio" },
            "command": server.command,
            "args": server.args,
            "url": server.url,
        }),
    })
}

#[must_use]
pub fn valid_mcp_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('.')
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-')
}
