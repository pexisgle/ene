use std::collections::HashMap;
use std::path::Path;

use ene_fiber::{ProfileRow, discover_plugin_executable_in, provider_plugin, task_seam};
use ene_kernel::{AiSettings, PluginProfileKind, PluginSettings, TaskBinding};
use ene_plane::Vault;
use ene_work::{McpServer, WorkError, WorkStore};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::CoreError;

#[must_use]
pub(crate) fn task_row_id(task: &str) -> String {
    format!("ai.tasks.{task}")
}

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

const PLUGIN_CONFIG_FILE: &str = "plugin-config.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PluginConfigFile {
    #[serde(default)]
    rows: HashMap<String, PersistedPluginConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedPluginConfig {
    #[serde(default)]
    values: Value,
    #[serde(default)]
    secret_keys: Vec<String>,
}

fn plugin_config_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join(PLUGIN_CONFIG_FILE)
}

fn vault_plugin_key(row_id: &str, field: &str) -> String {
    format!("plugin.config.{row_id}.{field}")
}

fn load_plugin_config_file(data_dir: &Path) -> Result<PluginConfigFile, CoreError> {
    let path = plugin_config_path(data_dir);
    if !path.exists() {
        return Ok(PluginConfigFile::default());
    }
    let raw = std::fs::read_to_string(&path)?;
    serde_json::from_str(&raw)
        .map_err(|err| CoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, err)))
}

fn save_plugin_config_file(data_dir: &Path, file: &PluginConfigFile) -> Result<(), CoreError> {
    let body = serde_json::to_string_pretty(file)
        .map_err(|err| CoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, err)))?;
    ene_config::config::atomic_write(&plugin_config_path(data_dir), &body)
        .map_err(|err| CoreError::Io(std::io::Error::other(err.to_string())))?;
    Ok(())
}

/// Overlay durable per-row settings (JSON + vault secrets) onto collected rows.
pub(crate) fn overlay_persisted_config(rows: &mut [ProfileRow], data_dir: &Path, vault: &Vault) {
    let file = match load_plugin_config_file(data_dir) {
        Ok(file) => file,
        Err(err) => {
            tracing::warn!(error = %err, "plugin-config.json unreadable");
            return;
        }
    };
    for row in rows {
        let Some(persisted) = file.rows.get(&row.row_id) else {
            continue;
        };
        row.config = merge_config(row.config.clone(), &persisted.values);
        inject_secrets(&mut row.config, &row.row_id, &persisted.secret_keys, vault);
    }
}

/// Persist applied settings. Secret fields go to the vault, never the JSON file.
pub(crate) fn persist_applied_plugin_config(
    data_dir: &Path,
    vault: &Vault,
    row_id: &str,
    values: &Value,
    secret_keys: &[String],
) -> Result<(), CoreError> {
    let mut public = values.clone();
    for key in secret_keys {
        let Some(secret) = take_path(&mut public, key) else {
            continue;
        };
        let Some(text) = secret.as_str().filter(|text| !text.is_empty()) else {
            continue;
        };
        drop(vault.put(&vault_plugin_key(row_id, key), text.as_bytes())?);
    }
    if !public.is_object() {
        public = json!({});
    }
    let mut file = load_plugin_config_file(data_dir)?;
    file.rows.insert(
        row_id.to_owned(),
        PersistedPluginConfig {
            values: public,
            secret_keys: secret_keys.to_vec(),
        },
    );
    save_plugin_config_file(data_dir, &file)
}

fn merge_config(base: Value, overlay: &Value) -> Value {
    let Value::Object(src) = overlay else {
        return base;
    };
    let mut dst = match base {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    for (key, value) in src {
        dst.insert(key.clone(), value.clone());
    }
    Value::Object(dst)
}

fn inject_secrets(config: &mut Value, row_id: &str, secret_keys: &[String], vault: &Vault) {
    for key in secret_keys {
        let Ok(bytes) = vault.export(&vault_plugin_key(row_id, key)) else {
            continue;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        set_path(config, key, Value::String(text));
    }
}

fn take_path(value: &mut Value, path: &str) -> Option<Value> {
    let mut parts = path.split('.').peekable();
    let mut current = value;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return current.as_object_mut()?.remove(part);
        }
        current = current.as_object_mut()?.get_mut(part)?;
    }
    None
}

fn set_path(value: &mut Value, path: &str, inserted: Value) {
    if !value.is_object() {
        *value = json!({});
    }
    let mut parts = path.split('.').peekable();
    let mut current = value;
    while let Some(part) = parts.next() {
        let Some(obj) = current.as_object_mut() else {
            return;
        };
        if parts.peek().is_none() {
            obj.insert(part.to_owned(), inserted);
            return;
        }
        let next = obj.entry(part.to_owned()).or_insert_with(|| json!({}));
        if !next.is_object() {
            *next = json!({});
        }
        current = next;
    }
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
    let capabilities = match plugin {
        "tool.fs" => vec![
            "fs.read".to_owned(),
            "fs.write".to_owned(),
            "fs.list".to_owned(),
            "fs.glob".to_owned(),
            "fs.delete".to_owned(),
        ],
        "tool.web" => vec!["net.fetch".to_owned()],
        _ => Vec::new(),
    };
    ProfileRow {
        row_id: plugin.to_owned(),
        plugin: plugin.to_owned(),
        requires: Vec::new(),
        capabilities,
        seams: Vec::new(),
        sandbox_required: needs_sandbox && binary.is_some(),
        config: serde_json::Value::Null,
    }
}

fn provider_rows(ai: &AiSettings, home: &Path) -> Vec<ProfileRow> {
    [
        ("chat", &ai.tasks.chat),
        ("classifier", &ai.tasks.classifier),
        ("embedding", &ai.tasks.embedding),
        ("proactive", &ai.tasks.proactive),
        ("tts", &ai.tasks.tts),
        ("stt", &ai.tasks.stt),
        ("approve", &ai.tasks.approve),
        ("job", &ai.tasks.job),
    ]
    .into_iter()
    .filter_map(|(task, binding)| {
        let row = provider_row(task, binding)?;
        if discover_plugin_executable_in(&row.plugin, Some(home)).is_none() {
            tracing::warn!(plugin = %row.plugin, "provider binary missing; skipping");
            return None;
        }
        Some(row)
    })
    .collect()
}

fn provider_row(task: &str, binding: &TaskBinding) -> Option<ProfileRow> {
    if binding.is_unconfigured() || !binding.plugin.starts_with("provider.") {
        return None;
    }
    let seam = task_seam(task)
        .or_else(|| provider_plugin(&binding.plugin).and_then(|meta| meta.seams.first().copied()))
        .unwrap_or("seam.llm");
    Some(ProfileRow {
        row_id: format!("ai.tasks.{task}"),
        plugin: binding.plugin.clone(),
        requires: Vec::new(),
        capabilities: Vec::new(),
        seams: vec![seam.to_owned()],
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
    })
}

fn mcp_rows(data_dir: &Path, servers: &[McpServer], home: &Path) -> Vec<ProfileRow> {
    if discover_plugin_executable_in("mcp.bridge", Some(home)).is_none() {
        if servers.iter().any(|server| server.enabled) {
            tracing::warn!("ene-tool-mcp missing; handwritten MCP rows skipped");
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
        seams: Vec::new(),
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
    use serde_json::{Value, json};
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
        let rows = harness_rows(
            PluginSettings {
                profile: "minimal".to_owned(),
                ..PluginSettings::default()
            }
            .kind(),
            Path::new(""),
        );
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
    fn web_harness_row_grants_net_fetch() {
        let rows = harness_rows(PluginProfileKind::Headless, Path::new(""));
        let web = rows
            .iter()
            .find(|row| row.plugin == "tool.web")
            .expect("tool.web");
        assert_eq!(web.capabilities, ["net.fetch"]);
    }

    #[test]
    fn fs_harness_row_grants_file_broker_caps() {
        let rows = harness_rows(PluginProfileKind::Headless, Path::new(""));
        let fs = rows
            .iter()
            .find(|row| row.plugin == "tool.fs")
            .expect("tool.fs");
        assert!(fs.capabilities.contains(&"fs.glob".to_owned()));
        assert!(fs.capabilities.contains(&"fs.delete".to_owned()));
    }

    #[test]
    fn resolved_home_defaults_to_data_plugins() {
        let settings = PluginSettings::default();
        assert_eq!(
            settings.resolved_home(Path::new("/tmp/ene-data")),
            Path::new("/tmp/ene-data/plugins")
        );
        let custom = PluginSettings {
            home_dir: "/opt/ene-plugins".to_owned(),
            ..PluginSettings::default()
        };
        assert_eq!(
            custom.resolved_home(Path::new("/tmp/ene-data")),
            Path::new("/opt/ene-plugins")
        );
    }

    #[test]
    fn chat_and_embedding_get_separate_provider_rows() {
        let chat = TaskBinding {
            plugin: "provider.gguf".to_owned(),
            model: "gemma-4-e2b".to_owned(),
            model_path: "/models/chat.gguf".to_owned(),
            ..TaskBinding::default()
        };
        let embedding = TaskBinding {
            plugin: "provider.gguf".to_owned(),
            model: "jina-v5-small".to_owned(),
            model_path: "/models/embed.gguf".to_owned(),
            ..TaskBinding::default()
        };
        let chat_row = super::provider_row("chat", &chat).expect("chat row");
        let embed_row = super::provider_row("embedding", &embedding).expect("embed row");
        assert_eq!(chat_row.row_id, "ai.tasks.chat");
        assert_eq!(embed_row.row_id, "ai.tasks.embedding");
        assert_eq!(chat_row.plugin, embed_row.plugin);
        assert_eq!(chat_row.seams, vec!["seam.llm".to_owned()]);
        assert_eq!(embed_row.seams, vec!["seam.embed".to_owned()]);
        assert_eq!(
            chat_row.config.get("model_path").and_then(Value::as_str),
            Some("/models/chat.gguf")
        );
        assert_eq!(
            embed_row.config.get("model_path").and_then(Value::as_str),
            Some("/models/embed.gguf")
        );
    }

    #[test]
    fn unconfigured_task_does_not_spawn_a_provider() {
        assert!(super::provider_row("chat", &TaskBinding::default()).is_none());
        assert!(super::provider_row("chat", &TaskBinding::echo()).is_none());
    }

    #[test]
    fn persist_strips_secrets_and_overlay_restores_them() {
        let dir = tempfile::TempDir::new().unwrap();
        let vault = Vault::open_or_create_keyfile(
            dir.path().join("vault.bin"),
            dir.path().join("vault.key"),
        )
        .unwrap();
        persist_applied_plugin_config(
            dir.path(),
            &vault,
            "tool.fs",
            &json!({
                "model": "fast",
                "api_key": "sk-live",
                "nested": { "token": "nested-secret", "keep": true }
            }),
            &["api_key".to_owned(), "nested.token".to_owned()],
        )
        .unwrap();
        let raw = std::fs::read_to_string(dir.path().join("plugin-config.json")).unwrap();
        assert!(raw.contains("fast"));
        assert!(raw.contains("keep"));
        assert!(!raw.contains("sk-live"));
        assert!(!raw.contains("nested-secret"));

        let mut rows = vec![ProfileRow {
            row_id: "tool.fs".to_owned(),
            plugin: "tool.fs".to_owned(),
            requires: Vec::new(),
            capabilities: Vec::new(),
            seams: Vec::new(),
            sandbox_required: false,
            config: Value::Null,
        }];
        overlay_persisted_config(&mut rows, dir.path(), &vault);
        assert_eq!(rows[0].config["model"], "fast");
        assert_eq!(rows[0].config["api_key"], "sk-live");
        assert_eq!(rows[0].config["nested"]["token"], "nested-secret");
        assert_eq!(rows[0].config["nested"]["keep"], json!(true));

        persist_applied_plugin_config(
            dir.path(),
            &vault,
            "tool.fs",
            &json!({ "model": "faster" }),
            &["api_key".to_owned(), "nested.token".to_owned()],
        )
        .unwrap();
        rows[0].config = Value::Null;
        overlay_persisted_config(&mut rows, dir.path(), &vault);
        assert_eq!(rows[0].config["model"], "faster");
        assert_eq!(rows[0].config["api_key"], "sk-live");
        assert_eq!(rows[0].config["nested"]["token"], "nested-secret");
    }
}
