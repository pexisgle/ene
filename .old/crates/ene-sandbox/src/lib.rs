//! # ene-sandbox
//!
//! OS-enforced sandbox for plugin / sidecar / stdio-MCP processes.
//!
//! The sandbox shows a process only:
//!
//! - read-only code and artifact directories,
//! - a size-capped dedicated temp directory,
//! - the broker IPC socket.
//!
//! Real `HOME`, `PATH`, working directories, credential areas, and user-file
//! areas are not passed through. On Linux the sandbox composes Landlock
//! (filesystem allowlist), `no_new_privs` + seccomp (privilege and dangerous
//! syscall filtering), rlimits, and — when the host can obtain them — cgroup
//! v2 and a network namespace. On Windows it composes a Job Object with
//! process/memory/CPU limits and kill-on-close (restricted-token /
//! `AppContainer` hardening is the documented next step, see `windows`).
//!
//! Every requirement is fail-closed: if a requirement is enabled and cannot
//! be initialized, the child is never exec'd and the spawn fails.

#![cfg_attr(
    all(test, target_os = "linux"),
    expect(
        clippy::expect_used,
        reason = "unit tests use expect for concise assertions"
    )
)]

mod error;
mod spec;

pub use error::SandboxError;
pub use spec::{CgroupSpec, ResourceLimits, SandboxSpec};

/// Applies the sandbox to a command's child process (Linux).
#[cfg(target_os = "linux")]
pub mod linux;

/// Applies the sandbox to a child process (Windows).
#[cfg(windows)]
pub mod windows;

/// Whether the current platform supports the sandbox primitives.
#[must_use]
pub fn supported() -> bool {
    #[cfg(target_os = "linux")]
    {
        linux::landlock_supported()
    }
    #[cfg(windows)]
    {
        true
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        false
    }
}
