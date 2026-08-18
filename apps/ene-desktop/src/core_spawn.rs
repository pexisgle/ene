use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const READY_TIMEOUT: Duration = Duration::from_secs(20);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);

pub struct CoreChild {
    child: Option<Child>,
    kill_on_drop: bool,
}

impl CoreChild {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            child: None,
            kill_on_drop: false,
        }
    }

    #[must_use]
    pub fn spawned(child: Child, kill_on_drop: bool) -> Self {
        Self {
            child: Some(child),
            kill_on_drop,
        }
    }
}

impl Drop for CoreChild {
    fn drop(&mut self) {
        if !self.kill_on_drop {
            drop(self.child.take());
            return;
        }
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

pub fn desktop_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ENE_DATA_DIR") {
        return PathBuf::from(dir);
    }
    ene_config::paths::app_data_dir()
}

fn core_bin_name() -> &'static str {
    if cfg!(windows) {
        "ene-core.exe"
    } else {
        "ene-core"
    }
}

pub(crate) fn binary_in_dir(dir: &Path) -> Option<PathBuf> {
    let candidate = dir.join(core_bin_name());
    candidate.is_file().then_some(candidate)
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
    if let Ok(mut path) = std::env::current_exe() {
        path.pop();
        if path.ends_with("deps") {
            path.pop();
        }
        if let Some(found) = binary_in_dir(&path) {
            return Some(found);
        }
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .find_map(|dir| binary_in_dir(&dir))
}

pub fn wait_for_api_json(path: &Path) -> Result<Value, String> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(mut value) = serde_json::from_str::<Value>(&text)
            && url_ready(&value)
        {
            if token_missing(&value)
                && let Some(token) = read_sibling_token(path, &value)
                && let Some(object) = value.as_object_mut()
            {
                object.insert("token".to_owned(), Value::String(token));
            }
            if !token_missing(&value) {
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

fn url_ready(value: &Value) -> bool {
    value
        .get("url")
        .and_then(Value::as_str)
        .is_some_and(|url| !url.is_empty())
}

fn token_missing(value: &Value) -> bool {
    value
        .get("token")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
}

fn read_sibling_token(api_json: &Path, value: &Value) -> Option<String> {
    let name = value
        .get("token_file")
        .and_then(Value::as_str)
        .unwrap_or("api.token");
    let token_path = api_json.parent()?.join(name);
    let token = std::fs::read_to_string(token_path).ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_owned())
}

pub fn spawn_core(data_dir: &Path) -> Result<Child, String> {
    let bin = ene_core_binary().ok_or_else(|| {
        "ene-core binary not found (build ene-core or set ENE_CORE_BIN)".to_owned()
    })?;
    std::fs::create_dir_all(data_dir).map_err(|err| err.to_string())?;
    let stale = data_dir.join("api.json");
    if stale.is_file() {
        std::fs::remove_file(&stale).map_err(|err| err.to_string())?;
    }
    Command::new(&bin)
        .arg("--data-dir")
        .arg(data_dir)
        .env("RUST_LOG", "error")
        .spawn()
        .map_err(|err| format!("failed to spawn ene-core: {err}"))
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

fn try_existing(
    runtime: &tokio::runtime::Handle,
    url: String,
    token: String,
) -> Option<(String, String)> {
    let client = ene_api::ApiClient::new(url.clone(), token.clone(), "desktop");
    runtime
        .block_on(health_reachable(&client, HEALTH_TIMEOUT))
        .ok()
        .map(|()| (url, token))
}

pub fn resolve_connection(
    runtime: &tokio::runtime::Handle,
    kill_on_drop: bool,
) -> Result<(String, String, CoreChild), String> {
    if let Some((url, token)) = env_api_config()
        && let Some(ready) = try_existing(runtime, url.clone(), token.clone())
    {
        return Ok((ready.0, ready.1, CoreChild::empty()));
    }

    let data_dir = desktop_data_dir();
    let ready_path = data_dir.join("api.json");
    if ready_path.is_file() {
        let raw = std::fs::read_to_string(&ready_path).unwrap_or_default();
        if let Ok(ready) = serde_json::from_str::<Value>(&raw)
            && let Ok((url, token)) = connection_from_ready(&ready)
            && let Some(connected) = try_existing(runtime, url, token)
        {
            return Ok((connected.0, connected.1, CoreChild::empty()));
        }
    }

    let child = spawn_core(&data_dir)?;
    let ready = wait_for_api_json(&ready_path)?;
    let (url, token) = connection_from_ready(&ready)?;
    let client = ene_api::ApiClient::new(url.clone(), token.clone(), "desktop");
    runtime
        .block_on(health_reachable(&client, HEALTH_TIMEOUT))
        .map_err(|err| format!("ene-core health check failed: {err}"))?;
    Ok((url, token, CoreChild::spawned(child, kill_on_drop)))
}
