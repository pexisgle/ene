use std::path::Path;

use ene_fiber::{ProfileRow, discover_plugin_executable_in};
use ene_kernel::{AiSettings, PluginProfileKind, PluginSettings};
use ene_work::{McpServer, WorkError, WorkStore};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct McpFile {
    #[serde(default)]
    servers: Vec<McpServer>,
}

pub fn collect_rows(
    data_dir: &Path,
    work: &WorkStore,
    ai: &AiSettings,
    plugins: &PluginSettings,
) -> Vec<ProfileRow> {
    let kind = plugins.kind();
    if !plugins.profile.is_empty() && plugins.profile != kind.as_str() {
        tracing::warn!(
            profile = %plugins.profile,
            "unknown plugins.profile; using desktop"
        );
    }
    let home = plugins.resolved_home(data_dir);
    let mut rows = harness_rows(kind, &home);
    rows.extend(provider_rows(ai, &home));
    if kind.includes_mcp() {
        rows.extend(mcp_rows(data_dir, &load_servers(data_dir, work), &home));
    }
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

fn harness_rows(kind: PluginProfileKind, home: &Path) -> Vec<ProfileRow> {
    kind.harness_plugins()
        .iter()
        .copied()
        .map(|plugin| harness_row(plugin, home))
        .collect()
}

fn harness_row(plugin: &str, home: &Path) -> ProfileRow {
    let binary = discover_plugin_executable_in(plugin, Some(home));
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

fn provider_rows(ai: &AiSettings, home: &Path) -> Vec<ProfileRow> {
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
        if discover_plugin_executable_in(&binding.plugin, Some(home)).is_none() {
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
                "server_path": binding.server_path,
                "cas_path": binding.cas_path,
                "model_path": binding.model_path,
                "server_args": binding.server_args,
                "startup_timeout_secs": binding.startup_timeout_secs,
            }),
        });
    }
    rows
}

fn mcp_rows(data_dir: &Path, servers: &[McpServer], home: &Path) -> Vec<ProfileRow> {
    if discover_plugin_executable_in("mcp.bridge", Some(home)).is_none() {
        if servers.iter().any(|server| server.enabled) {
            tracing::warn!("ene-harness-mcp missing; handwritten MCP rows skipped");
        }
        return Vec::new();
    }
    servers
        .iter()
        .filter(|server| server.enabled)
        .filter_map(|server| mcp_row(data_dir, server))
        .collect()
}

fn mcp_row(data_dir: &Path, server: &McpServer) -> Option<ProfileRow> {
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
            "skills_home": data_dir.join("skills"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use ene_kernel::PluginSettings;
    use std::path::Path;

    #[test]
    fn desktop_includes_app_and_mcp_eligible() {
        let kind = PluginProfileKind::Desktop;
        assert!(kind.harness_plugins().contains(&"tool.app"));
        assert!(kind.includes_mcp());
    }

    #[test]
    fn minimal_is_utility_only() {
        assert_eq!(
            PluginProfileKind::Minimal.harness_plugins(),
            &["tool.utility"]
        );
        assert!(!PluginProfileKind::Minimal.includes_mcp());
        let rows = harness_rows(PluginSettings::default().kind(), Path::new(""));
        assert!(rows.iter().any(|row| row.plugin == "tool.app"));
        let mut minimal = PluginSettings::default();
        minimal.profile = "minimal".to_owned();
        let rows = harness_rows(minimal.kind(), Path::new(""));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].plugin, "tool.utility");
    }

    #[test]
    fn headless_omits_app() {
        let plugins = PluginProfileKind::Headless.harness_plugins();
        assert!(plugins.contains(&"tool.fs"));
        assert!(!plugins.contains(&"tool.app"));
        assert!(PluginProfileKind::Headless.includes_mcp());
    }

    #[test]
    fn resolved_home_defaults_to_data_plugins() {
        let settings = PluginSettings::default();
        assert_eq!(
            settings.resolved_home(Path::new("/tmp/ene-data")),
            Path::new("/tmp/ene-data/plugins")
        );
        let mut custom = PluginSettings::default();
        custom.home_dir = "/opt/ene-plugins".to_owned();
        assert_eq!(
            custom.resolved_home(Path::new("/tmp/ene-data")),
            Path::new("/opt/ene-plugins")
        );
    }
}
