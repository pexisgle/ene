use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Resource ceilings applied via rlimits / cgroup / Job Object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum address space in bytes (`0` = no cap).
    pub max_address_space_bytes: u64,
    /// Maximum resident set in bytes (`0` = no cap).
    pub max_rss_bytes: u64,
    /// Maximum number of open file descriptors (`0` = no cap).
    pub max_fds: u64,
    /// Maximum number of processes for this user/process tree (`0` = no cap).
    ///
    /// NOTE: `RLIMIT_NPROC` counts **all** processes of the real user id, so
    /// a low value breaks forking on a busy desktop. Prefer the cgroup
    /// `pids.max` (per-tree) instead; this rlimit stays `0` by default.
    pub max_processes: u64,
    /// Maximum file size a child may write (`0` = no cap).
    pub max_file_size_bytes: u64,
    /// Maximum CPU time in seconds (`0` = no cap).
    pub max_cpu_seconds: u64,
    /// Maximum core dump size in bytes (`0` = no core dumps).
    pub max_core_bytes: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_address_space_bytes: 0,
            max_rss_bytes: 0,
            max_fds: 1024,
            max_processes: 0,
            max_file_size_bytes: 0,
            max_cpu_seconds: 0,
            max_core_bytes: 0,
        }
    }
}

/// What the plugin process may see, and which enforcement layers are on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxSpec {
    /// Read-only paths the plugin may access (binary dirs, system libs, CA
    /// roots, artifact dirs, …). Landlock allows exactly these.
    pub allowed_read_paths: Vec<PathBuf>,
    /// Writable paths (per-plugin temp dir, socket dir, approved writable
    /// dirs). Landlock allows write/create/remove here.
    pub allowed_write_paths: Vec<PathBuf>,
    /// Resource limits.
    pub limits: ResourceLimits,
    /// Apply Landlock (Linux). Mandatory when `true`.
    pub landlock: bool,
    /// Apply the seccomp dangerous-syscall filter (Linux).
    pub seccomp: bool,
    /// Set `no_new_privs` (Linux).
    pub no_new_privs: bool,
    /// Join/place the child in a fresh network namespace (Linux; requires
    /// privileges). When `true` the plugin has no direct network at all.
    pub network_namespace: bool,
    /// cgroup v2 memory/pids/cpu limits (Linux; requires a delegated
    /// cgroupfs). When `Some`, the child is moved into the named subgroup.
    pub cgroup: Option<CgroupSpec>,
    /// Apply the Windows Job Object (process/memory/CPU limits, tree
    /// kill-on-close).
    pub job_object: bool,
}

impl SandboxSpec {
    /// A spec with no enforcement layers (used for hosts that opted out).
    #[must_use]
    pub fn unrestricted() -> Self {
        Self {
            allowed_read_paths: Vec::new(),
            allowed_write_paths: Vec::new(),
            limits: ResourceLimits::default(),
            landlock: false,
            seccomp: false,
            no_new_privs: false,
            network_namespace: false,
            cgroup: None,
            job_object: false,
        }
    }

    /// Whether any enforcement layer is enabled.
    #[must_use]
    pub fn is_enforced(&self) -> bool {
        self.landlock
            || self.seccomp
            || self.no_new_privs
            || self.network_namespace
            || self.cgroup.is_some()
            || self.job_object
    }
}

/// cgroup v2 limits for one plugin process tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CgroupSpec {
    /// Memory limit in bytes.
    pub memory_max_bytes: u64,
    /// PIDs cap for the subgroup.
    pub pids_max: u64,
    /// CPU quota as `max` (e.g. `"50000 100000"` = 50% of one core) or
    /// `"max"` for unlimited.
    pub cpu_max: String,
}

impl Default for CgroupSpec {
    fn default() -> Self {
        Self {
            memory_max_bytes: 2 * 1024 * 1024 * 1024,
            pids_max: 64,
            cpu_max: "max".to_string(),
        }
    }
}
