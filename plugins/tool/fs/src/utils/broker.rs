//! Host-mediated file/process broker client for the fs plugin.
//!
//! All user-file I/O and process spawns go through the host's `file` and
//! `process` broker passengers; the plugin never touches the OS directly.
//! The host resolves paths against the plugin's grants, enforces approvals,
//! size caps, and the implicit-download ban, and audits every decision.
//!
//! In `cfg(test)` builds the transport is a local `std::fs` implementation
//! so the parsing/formatting action tests keep running without a host; the
//! shipped binary only ever talks to the broker.

use std::path::Path;
use std::sync::Arc;

use ene_plugin_broker::{BrokerClient, BrokerRequest, BrokerResponse};
use ene_plugin_proto::{HostServiceId, ToolError};
use parking_lot::RwLock;
use tokio::sync::Mutex;

/// File metadata mirror for broker results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMeta {
    /// Whether the entry is a directory.
    pub is_dir: bool,
    /// Size in bytes (0 for directories).
    pub size: u64,
    /// Last-modified Unix milliseconds (0 when unknown).
    pub modified_ms: u64,
}

/// Outcome of a read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOutcome {
    /// Bytes read (possibly truncated at the cap).
    pub data: Vec<u8>,
    /// Whether the file is larger than the requested cap.
    pub truncated: bool,
}

/// One directory entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// Entry name.
    pub name: String,
    /// Full path as requested.
    pub path: String,
    /// Whether the entry is a directory.
    pub is_dir: bool,
}

/// Process spawn outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutcome {
    /// Host-assigned pid.
    pub pid: u32,
    /// Exit code when the process finished, else `None`.
    pub exit_code: Option<i32>,
    /// Captured stdout (size-capped by the host).
    pub stdout: String,
    /// Captured stderr (size-capped by the host).
    pub stderr: String,
}

/// The broker channel shared by every fs action.
pub struct FileBroker {
    socket: RwLock<Option<String>>,
    token: RwLock<Option<String>>,
    client: Mutex<Option<BrokerClient>>,
    local: bool,
}

impl std::fmt::Debug for FileBroker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileBroker").finish_non_exhaustive()
    }
}

impl Default for FileBroker {
    fn default() -> Self {
        Self {
            socket: RwLock::new(None),
            token: RwLock::new(None),
            client: Mutex::new(None),
            local: cfg!(test),
        }
    }
}

impl FileBroker {
    /// A broker that talks to the host (production default).
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Captures the broker socket and auth token from the host sandbox
    /// config (protocol v8).
    pub fn configure(&self, socket: Option<&str>, token: Option<&str>) {
        self.socket.write().clone_from(&socket.map(str::to_string));
        self.token.write().clone_from(&token.map(str::to_string));
    }

    /// Whether this broker is configured to reach a host.
    pub fn is_configured(&self) -> bool {
        self.local || self.socket.read().is_some()
    }

    async fn client(
        &self,
        service: HostServiceId,
    ) -> Result<MutexGuard<'_, Option<BrokerClient>>, ToolError> {
        let mut client = self.client.lock().await;
        if client.is_none() {
            let socket = self.socket.read().clone();
            let token = self.token.read().clone();
            let (Some(socket), Some(token)) = (socket, token) else {
                return Err(ToolError::execution_failed(
                    "broker channel is not configured (missing broker socket/token from the host)",
                ));
            };
            *client = Some(
                BrokerClient::connect(Path::new(&socket), &token, service)
                    .await
                    .map_err(|e| {
                        ToolError::execution_failed(format!("broker connect failed: {e}"))
                    })?,
            );
        }
        Ok(client)
    }

    async fn request(
        &self,
        service: HostServiceId,
        request: BrokerRequest,
    ) -> Result<BrokerResponse, ToolError> {
        let mut client = self.client(service).await?;
        let Some(client) = client.as_mut() else {
            return Err(ToolError::execution_failed("broker client not initialized"));
        };
        client
            .request(&request)
            .await
            .map_err(|e| ToolError::execution_failed(format!("broker request failed: {e}")))
    }

    /// Reads a file (absolute or grant-resolved path).
    pub async fn read(&self, path: &str, max_bytes: u64) -> Result<ReadOutcome, ToolError> {
        if self.local {
            let data = std::fs::read(path)
                .map_err(|e| ToolError::execution_failed(format!("Cannot read {path}: {e}")))?;
            let truncated = u64::try_from(data.len()).unwrap_or(u64::MAX) > max_bytes;
            return Ok(ReadOutcome { data, truncated });
        }
        match self
            .request(
                HostServiceId::File,
                BrokerRequest::FileRead {
                    path: path.to_string(),
                    max_bytes: Some(max_bytes),
                },
            )
            .await?
        {
            BrokerResponse::FileReadOk {
                data,
                size,
                truncated,
            } => Ok(ReadOutcome {
                data,
                truncated: truncated || size > max_bytes,
            }),
            other => Err(ToolError::execution_failed(format!(
                "unexpected broker response: {other:?}"
            ))),
        }
    }

    /// Reads a UTF-8 text file.
    pub async fn read_text(&self, path: &str, max_bytes: u64) -> Result<String, ToolError> {
        let outcome = self.read(path, max_bytes).await?;
        String::from_utf8(outcome.data)
            .map_err(|e| ToolError::execution_failed(format!("Cannot read {path} as UTF-8: {e}")))
    }

    /// Writes bytes to a file.
    pub async fn write(
        &self,
        path: &str,
        data: Vec<u8>,
        create: bool,
        truncate: bool,
    ) -> Result<(), ToolError> {
        if self.local {
            let mut options = std::fs::OpenOptions::new();
            options.write(true);
            if truncate {
                options.truncate(true);
            }
            if create {
                options.create(true);
            }
            use std::io::Write;
            let mut file = options
                .open(path)
                .map_err(|e| ToolError::execution_failed(format!("Cannot write {path}: {e}")))?;
            file.write_all(&data)
                .map_err(|e| ToolError::execution_failed(format!("Cannot write {path}: {e}")))?;
            return Ok(());
        }
        match self
            .request(
                HostServiceId::File,
                BrokerRequest::FileWrite {
                    path: path.to_string(),
                    data,
                    create,
                    truncate,
                },
            )
            .await?
        {
            BrokerResponse::FileWriteOk { .. } => Ok(()),
            other => Err(ToolError::execution_failed(format!(
                "unexpected broker response: {other:?}"
            ))),
        }
    }

    /// Deletes a file (or a directory tree when `recursive`).
    pub async fn delete(&self, path: &str, recursive: bool) -> Result<(), ToolError> {
        if self.local {
            let result = if recursive {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };
            return result
                .map_err(|e| ToolError::execution_failed(format!("Cannot delete {path}: {e}")));
        }
        match self
            .request(
                HostServiceId::File,
                BrokerRequest::FileDelete {
                    path: path.to_string(),
                    recursive,
                },
            )
            .await?
        {
            BrokerResponse::FileDeleteOk => Ok(()),
            other => Err(ToolError::execution_failed(format!(
                "unexpected broker response: {other:?}"
            ))),
        }
    }

    /// Creates a directory (with parents when `recursive`).
    pub async fn create_dir(&self, path: &str, recursive: bool) -> Result<(), ToolError> {
        if self.local {
            let result = if recursive {
                std::fs::create_dir_all(path)
            } else {
                std::fs::create_dir(path)
            };
            return result.map_err(|e| {
                ToolError::execution_failed(format!("Cannot create directory {path}: {e}"))
            });
        }
        match self
            .request(
                HostServiceId::File,
                BrokerRequest::FileCreateDir {
                    path: path.to_string(),
                    recursive,
                },
            )
            .await?
        {
            BrokerResponse::FileCreateDirOk => Ok(()),
            other => Err(ToolError::execution_failed(format!(
                "unexpected broker response: {other:?}"
            ))),
        }
    }

    /// Lists a directory.
    pub async fn list(&self, path: &str) -> Result<Vec<DirEntry>, ToolError> {
        if self.local {
            let mut entries = Vec::new();
            for entry in std::fs::read_dir(path)
                .map_err(|e| ToolError::execution_failed(format!("Cannot list {path}: {e}")))?
            {
                let entry = entry
                    .map_err(|e| ToolError::execution_failed(format!("Cannot list {path}: {e}")))?;
                let file_type = entry
                    .file_type()
                    .map_err(|e| ToolError::execution_failed(format!("Cannot list {path}: {e}")))?;
                entries.push(DirEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    path: entry.path().to_string_lossy().into_owned(),
                    is_dir: file_type.is_dir(),
                });
            }
            return Ok(entries);
        }
        match self
            .request(
                HostServiceId::File,
                BrokerRequest::FileList {
                    path: path.to_string(),
                },
            )
            .await?
        {
            BrokerResponse::FileListOk { entries } => Ok(entries
                .into_iter()
                .map(|entry| DirEntry {
                    name: entry.name,
                    path: entry.path,
                    is_dir: entry.is_dir,
                })
                .collect()),
            other => Err(ToolError::execution_failed(format!(
                "unexpected broker response: {other:?}"
            ))),
        }
    }

    /// Stats a path; `None` when absent.
    pub async fn stat(&self, path: &str) -> Result<Option<FileMeta>, ToolError> {
        if self.local {
            return match std::fs::metadata(path) {
                Ok(metadata) => Ok(Some(FileMeta {
                    is_dir: metadata.is_dir(),
                    size: metadata.len(),
                    modified_ms: metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_millis().try_into().unwrap_or(u64::MAX)),
                })),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(ToolError::execution_failed(format!(
                    "Cannot stat {path}: {e}"
                ))),
            };
        }
        match self
            .request(
                HostServiceId::File,
                BrokerRequest::FileStat {
                    path: path.to_string(),
                },
            )
            .await?
        {
            BrokerResponse::FileStatOk { entry } => Ok(entry.map(|entry| FileMeta {
                is_dir: entry.is_dir,
                size: entry.size,
                modified_ms: entry.modified_ms,
            })),
            other => Err(ToolError::execution_failed(format!(
                "unexpected broker response: {other:?}"
            ))),
        }
    }

    /// Moves/renames a path.
    pub async fn move_path(&self, from: &str, to: &str) -> Result<(), ToolError> {
        if self.local {
            return std::fs::rename(from, to).map_err(|e| {
                ToolError::execution_failed(format!("Cannot move {from} to {to}: {e}"))
            });
        }
        match self
            .request(
                HostServiceId::File,
                BrokerRequest::FileMove {
                    from: from.to_string(),
                    to: to.to_string(),
                },
            )
            .await?
        {
            BrokerResponse::FileMoveOk => Ok(()),
            other => Err(ToolError::execution_failed(format!(
                "unexpected broker response: {other:?}"
            ))),
        }
    }

    /// Spawns a process through the host.
    pub async fn spawn_process(
        &self,
        argv: Vec<String>,
        cwd: Option<String>,
        env: Vec<(String, String)>,
        timeout_ms: u64,
        max_output_bytes: u64,
    ) -> Result<ProcessOutcome, ToolError> {
        if self.local {
            let mut command = tokio::process::Command::new(&argv[0]);
            command.args(&argv[1..]);
            command.kill_on_drop(true);
            command.stdout(std::process::Stdio::piped());
            command.stderr(std::process::Stdio::piped());
            command.stdin(std::process::Stdio::null());
            if let Some(cwd) = cwd {
                command.current_dir(cwd);
            }
            for (key, value) in env {
                command.env(key, value);
            }
            let timeout = if timeout_ms == 0 {
                std::time::Duration::from_mins(2)
            } else {
                std::time::Duration::from_millis(timeout_ms)
            };
            let output = tokio::time::timeout(timeout, command.output())
                .await
                .map_err(|_| ToolError::execution_failed("Command timed out".to_string()))?
                .map_err(|e| {
                    ToolError::execution_failed(format!("Failed to execute command: {e}"))
                })?;
            return Ok(ProcessOutcome {
                pid: 0,
                exit_code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        match self
            .request(
                HostServiceId::Process,
                BrokerRequest::ProcessSpawn {
                    argv,
                    cwd,
                    env,
                    timeout_ms,
                    max_output_bytes,
                },
            )
            .await?
        {
            BrokerResponse::ProcessSpawnOk {
                pid,
                exit_code,
                stdout,
                stderr,
            } => Ok(ProcessOutcome {
                pid,
                exit_code,
                stdout,
                stderr,
            }),
            other => Err(ToolError::execution_failed(format!(
                "unexpected broker response: {other:?}"
            ))),
        }
    }
}

type MutexGuard<'a, T> = tokio::sync::MutexGuard<'a, T>;

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit tests use expect for assertions")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_transport_reads_writes_lists_and_deletes() {
        let broker = FileBroker::new();
        assert!(broker.is_configured());
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("notes.txt");
        let path_str = path.to_string_lossy().into_owned();
        broker
            .write(&path_str, b"hello".to_vec(), true, true)
            .await
            .expect("write");
        assert_eq!(
            broker.read_text(&path_str, 1024).await.expect("read"),
            "hello"
        );
        let meta = broker.stat(&path_str).await.expect("stat").expect("exists");
        assert!(!meta.is_dir);
        assert_eq!(meta.size, 5);
        let entries = broker
            .list(&dir.path().to_string_lossy())
            .await
            .expect("list");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "notes.txt");
        broker.delete(&path_str, false).await.expect("delete");
        assert!(broker.stat(&path_str).await.expect("stat").is_none());
    }

    #[tokio::test]
    async fn local_transport_handles_directories_and_moves() {
        let broker = FileBroker::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let sub = dir.path().join("a").join("b");
        let sub_str = sub.to_string_lossy().into_owned();
        broker
            .create_dir(&sub_str, true)
            .await
            .expect("create_dir_all");
        let file = sub.join("f.txt");
        broker
            .write(&file.to_string_lossy(), b"x".to_vec(), true, true)
            .await
            .expect("write");
        let moved = dir.path().join("moved.txt");
        broker
            .move_path(&file.to_string_lossy(), &moved.to_string_lossy())
            .await
            .expect("move");
        assert!(!file.exists());
        assert!(moved.exists());
        broker
            .delete(&sub_str, true)
            .await
            .expect("recursive delete");
        assert!(!sub.exists());
    }
}
