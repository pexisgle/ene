use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const READY_TIMEOUT: Duration = Duration::from_secs(20);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);

pub struct CoreChild {
    child: Option<Child>,
}

impl CoreChild {
    #[must_use]
    pub fn empty() -> Self {
        Self { child: None }
    }

    #[must_use]
    pub fn spawned(child: Child) -> Self {
        Self { child: Some(child) }
    }
}

impl Drop for CoreChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if child.kill().is_err() {
                // Already exited.
            }
            drop(child.wait());
        }
    }
}

pub fn env_api_config() -> Option<(String, String)> {
    let Ok(url) = std::env::var("ENE_API_URL") else {
        return None;
    };
    if url.trim().is_empty() || url.ends_with(":0") {
        return None;
    }
    let token = std::env::var("ENE_API_TOKEN").unwrap_or_default();
    Some((url.trim_end_matches('/').to_owned(), token))
}

pub fn stage_spawn_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ENE_DATA_DIR") {
        return PathBuf::from(dir);
    }
    std::env::temp_dir().join(format!("ene-stage-core-{}", uuid::Uuid::new_v4().simple()))
}

pub fn ene_core_binary() -> Option<PathBuf> {
    if let Some(path) = option_env!("CARGO_BIN_EXE_ene_core") {
        return Some(PathBuf::from(path));
    }
    if let Ok(path) = std::env::var("ENE_CORE_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let Ok(mut path) = std::env::current_exe() else {
        return None;
    };
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("ene-core");
    if path.is_file() {
        return Some(path);
    }
    None
}

pub fn wait_for_api_json(path: &Path) -> Result<Value, String> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(value) = serde_json::from_str::<Value>(&text)
        {
            let url = value.get("url").and_then(Value::as_str);
            let token = value.get("token").and_then(Value::as_str);
            if url.is_some() && token.is_some() {
                return Ok(value);
            }
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for api.json at {}",
                path.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub fn spawn_core(data_dir: &Path) -> Result<CoreChild, String> {
    let bin = ene_core_binary().ok_or_else(|| {
        "ene-core binary not found (build ene-core or set ENE_CORE_BIN)".to_owned()
    })?;
    std::fs::create_dir_all(data_dir).map_err(|err| err.to_string())?;
    let child = Command::new(&bin)
        .arg("--data-dir")
        .arg(data_dir)
        .env("RUST_LOG", "error")
        .spawn()
        .map_err(|err| format!("failed to spawn ene-core: {err}"))?;
    Ok(CoreChild::spawned(child))
}

pub fn connection_from_ready(value: &Value) -> Result<(String, String), String> {
    let url = value
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| "api.json missing url".to_owned())?
        .trim_end_matches('/')
        .to_owned();
    let token = value
        .get("token")
        .and_then(Value::as_str)
        .ok_or_else(|| "api.json missing token".to_owned())?
        .to_owned();
    Ok((url, token))
}

pub async fn health_reachable(
    client: &ene_api::ApiClient,
    timeout: Duration,
) -> Result<(), ene_api::ApiError> {
    let deadline = Instant::now() + timeout;
    loop {
        match client.health().await {
            Ok(_) => return Ok(()),
            Err(err) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
                if Instant::now() >= deadline {
                    return Err(err);
                }
            }
            Err(err) => return Err(err),
        }
    }
}

pub fn resolve_connection(
    runtime: &tokio::runtime::Runtime,
) -> Result<(String, String, CoreChild), String> {
    if let Some((url, token)) = env_api_config() {
        let client = ene_api::ApiClient::new(url.clone(), token.clone(), "stage");
        match runtime.block_on(health_reachable(&client, HEALTH_TIMEOUT)) {
            Ok(()) => return Ok((url, token, CoreChild::empty())),
            Err(err) => {
                eprintln!("ene-stage: ENE_API_URL unreachable ({err}); spawning ene-core");
            }
        }
    }

    let data_dir = stage_spawn_data_dir();
    let child = spawn_core(&data_dir)?;
    let ready_path = data_dir.join("api.json");
    let ready = wait_for_api_json(&ready_path)?;
    let (url, token) = connection_from_ready(&ready)?;
    let client = ene_api::ApiClient::new(url.clone(), token.clone(), "stage");
    runtime
        .block_on(health_reachable(&client, HEALTH_TIMEOUT))
        .map_err(|err| format!("ene-core health check failed: {err}"))?;
    Ok((url, token, child))
}
