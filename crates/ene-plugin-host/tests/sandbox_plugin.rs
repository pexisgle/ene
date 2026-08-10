//! End-to-end smoke test: a real Rust binary starts under the OS sandbox.
//!
//! Spawns the package's `sandbox_fixture` binary with the same Landlock
//! allowlist the host computes (binary/lib dirs, CA roots, assets, temp,
//! socket dir) and proves the process survives past exec — a missing
//! allowlist entry would fail the exec and exit immediately.

#![cfg(target_os = "linux")]
#![expect(
    clippy::expect_used,
    reason = "integration test uses expect for concise assertions"
)]

use std::os::unix::process::CommandExt;
use std::time::Duration;

#[test]
fn real_binary_starts_under_the_os_sandbox() {
    if !ene_sandbox::supported() {
        return;
    }
    let binary = std::path::PathBuf::from(env!("CARGO_BIN_EXE_sandbox_fixture"));
    let temp = tempfile::tempdir().expect("tempdir");
    let socket_dir = temp.path().join("sockets");
    let socket = socket_dir.join("fixture.sock");
    std::fs::create_dir_all(&socket_dir).expect("socket dir");

    let mut read_paths = ene_sandbox::linux::default_read_paths(&binary);
    if let Some(path) = std::env::var_os("PATH") {
        read_paths.extend(std::env::split_paths(&path));
    }
    let mut write_paths = ene_sandbox::linux::default_write_paths();
    write_paths.push(socket_dir.clone());
    let spec = ene_sandbox::SandboxSpec {
        allowed_read_paths: read_paths,
        allowed_write_paths: write_paths,
        limits: ene_sandbox::ResourceLimits::default(),
        landlock: true,
        seccomp: true,
        no_new_privs: true,
        network_namespace: false,
        cgroup: None,
        job_object: false,
    };

    let mut command = std::process::Command::new(&binary);
    command
        .env("ENE_PLUGIN_SOCKET", &socket)
        .env("TMPDIR", temp.path());
    // SAFETY: the closure runs in the forked child before exec and only
    // touches process-local state (see ene-sandbox).
    unsafe {
        command.pre_exec(ene_sandbox::linux::pre_exec_closure(spec));
    }
    let mut child = command.spawn().expect("spawn under sandbox");
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        matches!(child.try_wait(), Ok(None)),
        "binary must still be running after exec; the sandbox allowlist broke it"
    );
    drop(child.kill());
    drop(child.wait());
}
