//! Engine preset / config-file generation.
//!
//! Sidecar engines that can serve multiple models from one process (e.g.
//! llama-server `--models-preset`) read a preset file from the work
//! directory. The generic writer serializes each profile as a JSON object;
//! engines with a different schema adapt this function.

use std::path::Path;

use super::config::SidecarConfig;

/// Writes the engine preset file and returns its path.
pub fn write_presets(
    work_dir: &Path,
    config: &SidecarConfig,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let presets: serde_json::Map<String, serde_json::Value> = config
        .profiles
        .iter()
        .map(|profile| {
            let mut entry = serde_json::Map::new();
            entry.insert(
                "model_path".to_string(),
                serde_json::json!(profile.model_path),
            );
            entry.insert(
                "gpu_layers".to_string(),
                serde_json::json!(profile.gpu_layers),
            );
            if let Some(context_size) = profile.context_size {
                entry.insert(
                    "context_size".to_string(),
                    serde_json::json!(context_size),
                );
            }
            (profile.name.clone(), serde_json::Value::Object(entry))
        })
        .collect();
    let path = work_dir.join("preset.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&serde_json::Value::Object(presets))?,
    )?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_presets_from_profiles() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = SidecarConfig {
            profiles: vec![crate::sidecar::config::SidecarProfile {
                name: "chat".to_string(),
                model_path: "/data/chat.gguf".to_string(),
                gpu_layers: "auto".to_string(),
                context_size: Some(4096),
            }],
            ..SidecarConfig::default()
        };
        let path = write_presets(dir.path(), &config).expect("presets");
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).expect("read")).expect("parse");
        assert_eq!(json["chat"]["model_path"], "/data/chat.gguf");
        assert_eq!(json["chat"]["gpu_layers"], "auto");
        assert_eq!(json["chat"]["context_size"], 4096);
    }
}
