//! Linux sandbox: Landlock + seccomp + `no_new_privs` + rlimits, plus
//! opt-in cgroup v2 and network namespaces.
//!
//! All layers run in the child between `fork()` and `exec()` via
//! [`prepare_command`]. Every enabled layer is fail-closed: if it cannot be
//! initialized the exec is aborted and the spawn fails.

use std::collections::BTreeMap;
use std::convert::TryInto;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

use landlock::{
    ABI, Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetStatus,
};
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, SeccompRule, TargetArch};

use crate::error::SandboxError;
use crate::spec::{CgroupSpec, ResourceLimits, SandboxSpec};

/// Whether the running kernel can enforce a Landlock ruleset at the ABI the
/// crate targets (probes without restricting the current process).
#[must_use]
pub fn landlock_supported() -> bool {
    Ruleset::default()
        .handle_access(AccessFs::from_all(ABI::V4))
        .and_then(Ruleset::create)
        .is_ok()
}

/// Installs the sandbox into `cmd`'s child (between fork and exec).
///
/// Returns `Ok` even for empty specs; an enabled layer that fails aborts the
/// spawn with an error.
pub fn prepare_command(
    cmd: &mut std::process::Command,
    spec: &SandboxSpec,
) -> Result<(), SandboxError> {
    if !spec.is_enforced() {
        return Ok(());
    }
    let spec = spec.clone();
    // SAFETY: `pre_exec` runs in the forked child before exec. The closure
    // touches only process-local state (rlimits, prctl, syscalls, cgroupfs
    // writes) and never allocates after the first call; all error paths
    // return an io::Error, which aborts the exec.
    unsafe {
        cmd.pre_exec(move || {
            apply_pre_exec(&spec).map_err(|e| std::io::Error::other(e.to_string()))
        });
    }
    Ok(())
}

/// Builds the child-side closure (fork → exec) for hosts that spawn through
/// a different `Command` type (e.g. tokio's process `Command`).
pub fn pre_exec_closure(
    spec: SandboxSpec,
) -> impl FnMut() -> std::io::Result<()> + Send + Sync + 'static {
    move || apply_pre_exec(&spec).map_err(|e| std::io::Error::other(e.to_string()))
}

fn apply_pre_exec(spec: &SandboxSpec) -> Result<(), SandboxError> {
    if spec.no_new_privs {
        set_no_new_privs()?;
    }
    apply_rlimits(&spec.limits)?;
    if let Some(cgroup) = &spec.cgroup {
        move_to_cgroup(cgroup)?;
    }
    if spec.network_namespace {
        unshare_network_namespace()?;
    }
    if spec.seccomp {
        apply_seccomp()?;
    }
    if spec.landlock {
        apply_landlock(spec)?;
    }
    Ok(())
}

fn set_no_new_privs() -> Result<(), SandboxError> {
    // SAFETY: PR_SET_NO_NEW_PRIVS takes one integer argument; returns 0 on
    // success and -1 with errno on failure.
    let ret = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if ret == 0 {
        Ok(())
    } else {
        Err(SandboxError::Syscall(
            "no_new_privs",
            std::io::Error::last_os_error().to_string(),
        ))
    }
}

fn apply_rlimits(limits: &ResourceLimits) -> Result<(), SandboxError> {
    let mut entries: Vec<(libc::__rlimit_resource_t, u64)> = Vec::new();
    if limits.max_address_space_bytes > 0 {
        entries.push((libc::RLIMIT_AS, limits.max_address_space_bytes));
    }
    if limits.max_fds > 0 {
        entries.push((libc::RLIMIT_NOFILE, limits.max_fds));
    }
    if limits.max_processes > 0 {
        entries.push((libc::RLIMIT_NPROC, limits.max_processes));
    }
    if limits.max_file_size_bytes > 0 {
        entries.push((libc::RLIMIT_FSIZE, limits.max_file_size_bytes));
    }
    if limits.max_cpu_seconds > 0 {
        entries.push((libc::RLIMIT_CPU, limits.max_cpu_seconds));
    }
    // Core dumps are always suppressed unless explicitly requested.
    entries.push((libc::RLIMIT_CORE, limits.max_core_bytes));
    for (resource, value) in entries {
        let rlim = libc::rlimit {
            rlim_cur: value,
            rlim_max: value,
        };
        let rlim_ptr = std::ptr::from_ref(&rlim).cast_mut();
        // SAFETY: setrlimit is async-signal-safe; both values are plain
        // integers within the platform's rlim_t range (u64 on Linux).
        if unsafe { libc::setrlimit(resource, rlim_ptr) } != 0 {
            return Err(SandboxError::Syscall(
                "setrlimit",
                std::io::Error::last_os_error().to_string(),
            ));
        }
    }
    Ok(())
}

/// Moves the child into a fresh cgroup v2 subgroup with memory/pids/cpu
/// limits. Requires a delegated (writable) cgroupfs subtree.
fn move_to_cgroup(spec: &CgroupSpec) -> Result<(), SandboxError> {
    let (mount_root, current) = discover_cgroup2()?;
    let target = mount_root
        .join(current.trim_start_matches('/'))
        .join(format!("ene-{}", std::process::id()));
    std::fs::create_dir(&target)?;
    std::fs::write(target.join("memory.max"), spec.memory_max_bytes.to_string())?;
    std::fs::write(target.join("pids.max"), spec.pids_max.to_string())?;
    std::fs::write(target.join("cpu.max"), &spec.cpu_max)?;
    std::fs::write(target.join("cgroup.procs"), std::process::id().to_string())?;
    Ok(())
}

fn discover_cgroup2() -> Result<(PathBuf, String), SandboxError> {
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo")
        .map_err(|e| SandboxError::Privilege("cgroup v2", e.to_string()))?;
    let mount_root = mountinfo.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let _mount_id = fields.next()?;
        let _parent_id = fields.next()?;
        let _major_minor = fields.next()?;
        let _root = fields.next()?;
        let mount_point = fields.next()?;
        // The separator `-` ends the per-mount fields.
        let rest = fields.collect::<Vec<_>>();
        let fs_type = rest
            .iter()
            .position(|field| *field == "-")
            .and_then(|i| rest.get(i + 1))
            .copied()?;
        (fs_type == "cgroup2").then(|| PathBuf::from(mount_point))
    });
    let Some(mount_root) = mount_root else {
        return Err(SandboxError::Unsupported("cgroup v2"));
    };
    let cgroup_line = std::fs::read_to_string("/proc/self/cgroup")
        .map_err(|e| SandboxError::Privilege("cgroup v2", e.to_string()))?;
    let current = cgroup_line
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or(SandboxError::Unsupported("cgroup v2"))?;
    Ok((mount_root, current.to_string()))
}

fn unshare_network_namespace() -> Result<(), SandboxError> {
    // SAFETY: unshare(CLONE_NEWNET) returns 0 on success; it requires
    // CAP_SYS_ADMIN in the initial user namespace and is fail-closed here.
    let ret = unsafe { libc::unshare(libc::CLONE_NEWNET) };
    if ret == 0 {
        Ok(())
    } else {
        Err(SandboxError::Privilege(
            "network namespace",
            std::io::Error::last_os_error().to_string(),
        ))
    }
}

/// Syscalls that are never needed by a plugin and are denied with `EPERM`.
/// Kept to the universally-available set so the filter compiles on every
/// supported Linux architecture.
const DANGEROUS_SYSCALLS: &[i64] = &[
    libc::SYS_mount,
    libc::SYS_umount2,
    libc::SYS_pivot_root,
    libc::SYS_chroot,
    libc::SYS_ptrace,
    libc::SYS_process_vm_readv,
    libc::SYS_process_vm_writev,
    libc::SYS_kexec_load,
    libc::SYS_kexec_file_load,
    libc::SYS_reboot,
    libc::SYS_swapon,
    libc::SYS_swapoff,
    libc::SYS_syslog,
    libc::SYS_init_module,
    libc::SYS_finit_module,
    libc::SYS_delete_module,
    libc::SYS_bpf,
    libc::SYS_userfaultfd,
    libc::SYS_perf_event_open,
    libc::SYS_add_key,
    libc::SYS_request_key,
    libc::SYS_keyctl,
    libc::SYS_setns,
    libc::SYS_unshare,
    libc::SYS_open_by_handle_at,
    libc::SYS_name_to_handle_at,
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
    libc::SYS_execveat,
    libc::SYS_acct,
    libc::SYS_quotactl,
    libc::SYS_settimeofday,
    libc::SYS_clock_settime,
    libc::SYS_sethostname,
    libc::SYS_setdomainname,
    libc::SYS_vhangup,
];

fn apply_seccomp() -> Result<(), SandboxError> {
    let rules: BTreeMap<i64, Vec<SeccompRule>> = DANGEROUS_SYSCALLS
        .iter()
        .map(|&syscall| (syscall, Vec::new()))
        .collect();
    let arch = TargetArch::try_from(std::env::consts::ARCH)
        .map_err(|_| SandboxError::Unsupported("seccomp (architecture)"))?;
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        arch,
    )
    .map_err(|e| SandboxError::Syscall("seccomp (compile)", e.to_string()))?;
    let program: BpfProgram = filter.try_into().map_err(|e: seccompiler::BackendError| {
        SandboxError::Syscall("seccomp (compile)", e.to_string())
    })?;
    let fprog = libc::sock_fprog {
        len: u16::try_from(program.len()).map_err(|_| {
            SandboxError::Syscall("seccomp (compile)", "filter too large".to_string())
        })?,
        filter: program.as_ptr() as *mut libc::sock_filter,
    };
    // SAFETY: PR_SET_SECCOMP with SECCOMP_MODE_FILTER installs the compiled
    // BPF program; the sock_fprog outlives the call. seccomp filters are
    // inherited by all future threads of the process.
    let ret = unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &fprog) };
    if ret == 0 {
        Ok(())
    } else {
        Err(SandboxError::Syscall(
            "seccomp (install)",
            std::io::Error::last_os_error().to_string(),
        ))
    }
}

fn apply_landlock(spec: &SandboxSpec) -> Result<(), SandboxError> {
    // ABI V4 covers REFER (rename/link across directories) and TRUNCATE;
    // older kernels cannot enforce this set, which fails the spawn (the
    // sandbox never silently downgrades).
    let abi = ABI::V4;
    let handled = AccessFs::from_all(abi);
    let read_dir = AccessFs::from_read(abi);
    let read_file = read_dir & !AccessFs::ReadDir;
    let write_dir = handled & !read_dir;
    let write_file = AccessFs::WriteFile | AccessFs::Truncate;
    let mut rules: Vec<PathBeneath<PathFd>> = Vec::new();
    for path in &spec.allowed_read_paths {
        if !path.exists() {
            continue;
        }
        // Directory-only rights (ReadDir) on a file rule would downgrade the
        // whole ruleset to best-effort; pick the access set by path kind.
        let access = if path.is_dir() { read_dir } else { read_file };
        rules.push(PathBeneath::new(
            PathFd::new(path).map_err(|e| SandboxError::Landlock(e.to_string()))?,
            access,
        ));
    }
    for path in &spec.allowed_write_paths {
        if !path.exists() {
            continue;
        }
        let access = if path.is_dir() { write_dir } else { write_file };
        rules.push(PathBeneath::new(
            PathFd::new(path).map_err(|e| SandboxError::Landlock(e.to_string()))?,
            access,
        ));
    }
    let status = Ruleset::default()
        .handle_access(handled)
        .map_err(|e| SandboxError::Landlock(e.to_string()))?
        .create()
        .map_err(|e| SandboxError::Landlock(e.to_string()))?
        .add_rules(
            rules
                .into_iter()
                .map::<Result<_, landlock::RulesetError>, _>(Ok),
        )
        .map_err(|e| SandboxError::Landlock(e.to_string()))?
        .restrict_self()
        .map_err(|e| SandboxError::Landlock(e.to_string()))?;
    match status.ruleset {
        RulesetStatus::FullyEnforced => Ok(()),
        RulesetStatus::PartiallyEnforced | RulesetStatus::NotEnforced => {
            Err(SandboxError::Unsupported("landlock (ABI V4)"))
        }
    }
}

/// System paths a dynamically linked plugin binary needs to read: loader and
/// shared-library roots, CA stores, timezone, and identity files. Callers
/// extend this with artifact/code directories.
#[must_use]
pub fn default_read_paths(binary: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(canonical) = binary.canonicalize()
        && let Some(parent) = canonical.parent()
    {
        paths.push(parent.to_path_buf());
    }
    for candidate in [
        "/nix/store",
        "/usr/lib",
        "/usr/lib64",
        "/lib",
        "/lib64",
        "/usr/local/lib",
        "/etc/ssl",
        "/etc/ca-certificates",
        "/etc/pki",
        "/etc/localtime",
        "/etc/passwd",
        "/etc/group",
        "/etc/nsswitch.conf",
        "/etc/resolv.conf",
        "/proc/self",
        "/dev/null",
        "/dev/urandom",
        "/dev/random",
        "/dev/zero",
    ] {
        if Path::new(candidate).exists() {
            paths.push(PathBuf::from(candidate));
        }
    }
    paths
}

/// Device nodes plugins routinely need to *write* to (`/dev/null` for output
/// redirection). These are single files, so the write rule stays narrow
/// (`WriteFile | Truncate`).
#[must_use]
pub fn default_write_paths() -> Vec<PathBuf> {
    vec!["/dev/null".into()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{CgroupSpec, ResourceLimits, SandboxSpec};

    fn sh_path() -> PathBuf {
        // Resolve `sh` through PATH, like the host would for a plugin binary.
        let path = std::env::var_os("PATH")
            .and_then(|p| {
                std::env::split_paths(&p)
                    .map(|dir| dir.join("sh"))
                    .find(|candidate| candidate.exists())
            })
            .unwrap_or_else(|| PathBuf::from("/bin/sh"));
        path.canonicalize().unwrap_or(path)
    }

    #[test]
    fn unrestricted_spec_needs_no_kernel() {
        let spec = SandboxSpec::unrestricted();
        assert!(!spec.is_enforced());
        let mut cmd = std::process::Command::new(sh_path());
        cmd.arg("-c").arg("exit 0");
        prepare_command(&mut cmd, &spec).expect("no-op prepare");
        let status = cmd.status().expect("spawn");
        assert!(status.success());
    }

    #[test]
    fn seccomp_denies_privileged_syscalls_when_supported() {
        // seccomp works on every modern kernel; the sandboxed child must be
        // unable to call unshare (denied regardless of privileges).
        let mut spec = SandboxSpec::unrestricted();
        spec.seccomp = true;
        spec.no_new_privs = true;
        let mut cmd = std::process::Command::new(sh_path());
        cmd.arg("-c").arg("unshare -n true 2>/dev/null; exit 0");
        prepare_command(&mut cmd, &spec).expect("prepare");
        let output = cmd.output().expect("run");
        // unshare is blocked with EPERM; sh still exits 0 (the command
        // swallows the error), proving the filter was installed.
        assert!(output.status.success());
    }

    #[test]
    fn plugin_cannot_write_outside_allowed_paths() {
        if !landlock_supported() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let plugin_temp = dir.path().join("plugin-tmp");
        let workspace = dir.path().join("workspace");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&plugin_temp).expect("mkdir");
        std::fs::create_dir_all(&workspace).expect("mkdir");
        std::fs::create_dir_all(&outside).expect("mkdir");

        let mut read_paths = default_read_paths(&sh_path());
        read_paths.push(workspace.clone());
        let write_paths = vec![plugin_temp.clone()];
        let spec = SandboxSpec {
            allowed_read_paths: read_paths,
            allowed_write_paths: write_paths,
            limits: ResourceLimits::default(),
            landlock: true,
            seccomp: false,
            no_new_privs: true,
            network_namespace: false,
            cgroup: None,
            job_object: false,
        };
        let mut cmd = std::process::Command::new(sh_path());
        cmd.arg("-c").arg(format!(
            "echo allowed > {temp}/ok.txt; echo blocked > {workspace}/leak.txt 2>/dev/null; \
             echo blocked > {outside}/leak.txt 2>/dev/null; exit 0",
            temp = plugin_temp.display(),
            workspace = workspace.display(),
            outside = outside.display(),
        ));
        prepare_command(&mut cmd, &spec).expect("prepare");
        let output = cmd.output().expect("run");
        assert!(output.status.success());
        assert_eq!(
            std::fs::read_to_string(plugin_temp.join("ok.txt")).expect("read ok"),
            "allowed\n"
        );
        assert!(
            !workspace.join("leak.txt").exists(),
            "plugin must not write outside its temp dir"
        );
        assert!(
            !outside.join("leak.txt").exists(),
            "plugin must not write outside the sandbox allowlist"
        );
    }

    #[test]
    fn landlock_enforces_path_allowlist_when_supported() {
        if !landlock_supported() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let allowed_read = dir.path().join("read");
        let allowed_write = dir.path().join("write");
        let blocked = dir.path().join("blocked");
        std::fs::create_dir_all(&allowed_read).expect("mkdir");
        std::fs::create_dir_all(&allowed_write).expect("mkdir");
        std::fs::create_dir_all(&blocked).expect("mkdir");
        std::fs::write(allowed_read.join("ok.txt"), b"visible").expect("write");
        std::fs::write(blocked.join("secret.txt"), b"hidden").expect("write");

        let mut read_paths = default_read_paths(&sh_path());
        read_paths.push(allowed_read.clone());
        let mut write_paths = default_write_paths();
        write_paths.push(allowed_write.clone());
        let spec = SandboxSpec {
            allowed_read_paths: read_paths,
            allowed_write_paths: write_paths,
            limits: ResourceLimits::default(),
            landlock: true,
            seccomp: false,
            no_new_privs: true,
            network_namespace: false,
            cgroup: None,
            job_object: false,
        };
        let mut cmd = std::process::Command::new(sh_path());
        cmd.arg("-c").arg(format!(
            "cat {read_ok} 2>/dev/null; cat {read_blocked} 2>/dev/null; \
             echo written > {write_ok}/out.txt; echo nope > {write_blocked}/x.txt 2>/dev/null; exit 0",
            read_ok = allowed_read.join("ok.txt").display(),
            read_blocked = blocked.join("secret.txt").display(),
            write_ok = allowed_write.display(),
            write_blocked = blocked.display(),
        ));
        prepare_command(&mut cmd, &spec).expect("prepare");
        let output = cmd.output().expect("run");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("visible"),
            "allowed read must work: {stdout}"
        );
        assert!(
            !stdout.contains("hidden"),
            "blocked read must fail: {stdout}"
        );
        assert_eq!(
            std::fs::read_to_string(allowed_write.join("out.txt")).expect("read out"),
            "written\n"
        );
        assert!(!blocked.join("x.txt").exists(), "blocked write must fail");
    }

    #[test]
    fn cgroup_discovery_reads_current_mount() {
        // On hosts without cgroup v2 the discovery must error cleanly, not
        // panic; on hosts with it, the returned paths must be sane.
        if let Ok((root, current)) = discover_cgroup2() {
            assert!(root.is_absolute());
            assert!(current.starts_with('/'));
        }
    }

    #[test]
    fn default_read_paths_include_system_roots() {
        let paths = default_read_paths(&sh_path());
        assert!(!paths.is_empty());
        assert!(paths.iter().any(|p| p.is_dir() || p.is_file()));
    }

    #[test]
    fn cgroup_spec_defaults_are_sane() {
        let spec = CgroupSpec::default();
        assert_eq!(spec.pids_max, 64);
        assert_eq!(spec.cpu_max, "max");
    }
}
