use std::path::{Path, PathBuf};
use std::sync::Arc;

use ene_action::WorkspaceRoot;
#[cfg(windows)]
use windows_sys::Win32::Foundation::HANDLE;

#[cfg(feature = "test-support")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StagingCleanupPause {
    pub entered: PathBuf,
    pub release: PathBuf,
}

#[derive(Default)]
pub(crate) struct StagingLeaseOptions {
    #[cfg(feature = "test-support")]
    pub pause: Option<StagingCleanupPause>,
    #[cfg(feature = "test-support")]
    pub fail_cleanup: bool,
}

pub(crate) struct StagingPreparationFailure {
    pub reason: String,
    pub lease: Option<Arc<StagingLease>>,
}

pub(crate) struct StagingCleanupFailure {
    pub reason: String,
    pub lease: Arc<StagingLease>,
}

impl core::fmt::Debug for StagingPreparationFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("StagingPreparationFailure")
            .field("reason", &self.reason)
            .field("has_lease", &self.lease.is_some())
            .finish()
    }
}

impl core::fmt::Debug for StagingCleanupFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("StagingCleanupFailure")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

pub(crate) struct StagingLease {
    object: StagingObject,
    workspace_root: WorkspaceRoot,
    original_path: PathBuf,
    options: StagingLeaseOptions,
}

pub(crate) async fn prepare_staging_lease(
    path: &Path,
    workspace_root: &WorkspaceRoot,
    options: StagingLeaseOptions,
) -> Result<Arc<StagingLease>, StagingPreparationFailure> {
    let path = path.to_path_buf();
    let workspace_root = workspace_root.clone();
    tokio::task::spawn_blocking(move || {
        let failure = |reason| StagingPreparationFailure {
            reason,
            lease: None,
        };
        let root = path
            .parent()
            .ok_or_else(|| failure(String::from("staging directory has no parent")))?;
        let parent = root
            .parent()
            .ok_or_else(|| failure(String::from("staging root has no parent")))?;
        if staging_path_contains_reparse(&path) {
            return Err(failure(String::from(
                "staging path contains a reparse point",
            )));
        }
        workspace_root
            .validate_staging_directory(parent)
            .map_err(failure)?;
        match std::fs::create_dir(root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = std::fs::symlink_metadata(root).map_err(|error| {
                    failure(format!("staging root metadata is unavailable: {error}"))
                })?;
                if metadata_is_reparse(&metadata) || !metadata.is_dir() {
                    return Err(failure(String::from(
                        "staging root is not a real directory",
                    )));
                }
            }
            Err(error) => return Err(failure(error.to_string())),
        }
        workspace_root
            .validate_staging_directory(root)
            .map_err(failure)?;
        if staging_path_contains_reparse(&path) {
            return Err(failure(String::from("staging path became a reparse point")));
        }
        workspace_root
            .validate_staging_directory(root)
            .map_err(failure)?;
        if let Err(error) = std::fs::create_dir(&path) {
            let reason = if error.kind() == std::io::ErrorKind::AlreadyExists {
                format!("staging path already exists: {error}")
            } else {
                error.to_string()
            };
            return Err(failure(reason));
        }
        let object = StagingObject::open(&path).map_err(failure)?;
        let lease = Arc::new(StagingLease {
            object,
            workspace_root,
            original_path: path.clone(),
            options,
        });
        if let Err(reason) = lease.object.set_private_directory_permissions() {
            return Err(StagingPreparationFailure {
                reason,
                lease: Some(lease),
            });
        }
        if let Err(reason) = lease
            .object
            .validate_current_path(&lease.workspace_root, &lease.original_path)
        {
            return Err(StagingPreparationFailure {
                reason,
                lease: Some(lease),
            });
        }
        Ok(lease)
    })
    .await
    .map_err(|error| StagingPreparationFailure {
        reason: format!("staging preparation task failed: {error}"),
        lease: None,
    })?
}

pub(crate) async fn cleanup_staging_lease(
    lease: Arc<StagingLease>,
) -> Result<(), StagingCleanupFailure> {
    let cleanup_lease = Arc::clone(&lease);
    tokio::task::spawn_blocking(move || {
        if cleanup_lease.options.test_cleanup_failure() {
            return Err(StagingCleanupFailure {
                reason: String::from("staging cleanup failed"),
                lease: cleanup_lease,
            });
        }
        if let Err(reason) = cleanup_lease
            .object
            .validate_current_path(&cleanup_lease.workspace_root, &cleanup_lease.original_path)
        {
            return Err(StagingCleanupFailure {
                reason,
                lease: cleanup_lease,
            });
        }
        if let Err(reason) = cleanup_lease.options.pause_cleanup() {
            return Err(StagingCleanupFailure {
                reason,
                lease: cleanup_lease,
            });
        }
        if let Err(reason) = cleanup_lease
            .object
            .validate_current_path(&cleanup_lease.workspace_root, &cleanup_lease.original_path)
        {
            return Err(StagingCleanupFailure {
                reason,
                lease: cleanup_lease,
            });
        }
        if let Err(reason) = cleanup_lease.object.remove_contents() {
            return Err(StagingCleanupFailure {
                reason,
                lease: cleanup_lease,
            });
        }
        if let Err(reason) = cleanup_lease
            .object
            .validate_current_path(&cleanup_lease.workspace_root, &cleanup_lease.original_path)
        {
            return Err(StagingCleanupFailure {
                reason,
                lease: cleanup_lease,
            });
        }
        if let Err(reason) = cleanup_lease
            .object
            .remove_directory_entry(&cleanup_lease.original_path)
        {
            return Err(StagingCleanupFailure {
                reason,
                lease: cleanup_lease,
            });
        }
        Ok(())
    })
    .await
    .map_err(|error| StagingCleanupFailure {
        reason: format!("staging cleanup task failed: {error}"),
        lease,
    })?
}

impl StagingLeaseOptions {
    fn test_cleanup_failure(&self) -> bool {
        #[cfg(feature = "test-support")]
        {
            self.fail_cleanup
        }
        #[cfg(not(feature = "test-support"))]
        {
            false
        }
    }

    fn pause_cleanup(&self) -> Result<(), String> {
        #[cfg(feature = "test-support")]
        {
            let Some(pause) = self.pause.as_ref() else {
                return Ok(());
            };
            std::fs::write(&pause.entered, b"cleanup-validated")
                .map_err(|error| format!("staging cleanup barrier is unavailable: {error}"))?;
            while !pause.release.exists() {
                std::thread::yield_now();
            }
        }
        Ok(())
    }
}

fn metadata_is_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn staging_path_contains_reparse(path: &Path) -> bool {
    let mut current = Some(path);
    while let Some(candidate) = current {
        match std::fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata_is_reparse(&metadata) => return true,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return true,
        }
        current = candidate.parent();
    }
    false
}

#[cfg(unix)]
struct StagingObject {
    directory: std::fs::File,
    identity: UnixStagingIdentity,
}

#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct UnixStagingIdentity {
    device: u64,
    inode: u64,
}

#[cfg(unix)]
impl StagingObject {
    fn open(path: &Path) -> Result<Self, String> {
        use std::os::unix::fs::OpenOptionsExt as _;

        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|error| format!("staging directory could not be opened: {error}"))?;
        let identity = directory_identity(&directory)?;
        Ok(Self {
            directory,
            identity,
        })
    }

    fn set_private_directory_permissions(&self) -> Result<(), String> {
        use std::os::fd::AsRawFd as _;

        // SAFETY: the descriptor is owned by `directory`; fchmod has no memory
        // safety preconditions and 0o700 is the intended directory mode.
        if unsafe { libc::fchmod(self.directory.as_raw_fd(), 0o700) } == 0 {
            Ok(())
        } else {
            Err(format!(
                "staging directory permissions could not be set: {}",
                std::io::Error::last_os_error()
            ))
        }
    }

    fn validate_current_path(
        &self,
        workspace_root: &WorkspaceRoot,
        original_path: &Path,
    ) -> Result<(), String> {
        let current = self.current_path(original_path)?;
        workspace_root.validate_staging_directory(&current)?;
        workspace_root.validate_staging_tree(&current)?;
        let metadata = std::fs::metadata(&current)
            .map_err(|error| format!("staging directory metadata is unavailable: {error}"))?;
        if UnixStagingIdentity::from_metadata(&metadata)? != self.identity {
            return Err(String::from(
                "staging path no longer identifies the owned directory",
            ));
        }
        Ok(())
    }

    fn remove_contents(&self) -> Result<(), String> {
        use std::os::fd::AsRawFd as _;

        remove_unix_directory_contents(self.directory.as_raw_fd())
    }

    fn remove_directory_entry(&self, original_path: &Path) -> Result<(), String> {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::ffi::OsStrExt as _;
        use std::os::unix::fs::OpenOptionsExt as _;

        let path = self.current_path(original_path)?;
        let name = path
            .file_name()
            .ok_or_else(|| String::from("owned staging directory has no name"))?;
        let parent = path
            .parent()
            .ok_or_else(|| String::from("owned staging directory has no parent"))?;
        let parent = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(parent)
            .map_err(|error| format!("staging directory parent is unavailable: {error}"))?;
        let name = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| String::from("staging directory name is invalid"))?;
        // SAFETY: libc::stat is a plain C data structure and an all-zero value
        // is a valid writable output buffer before fstatat fills it.
        let mut metadata = unsafe { std::mem::zeroed::<libc::stat>() };
        // SAFETY: `parent` owns a directory descriptor, `name` is NUL-terminated,
        // and `metadata` is writable for the duration of the call.
        if unsafe {
            libc::fstatat(
                parent.as_raw_fd(),
                name.as_ptr(),
                &mut metadata,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(format!(
                "owned staging directory entry could not be identified: {}",
                std::io::Error::last_os_error()
            ));
        }
        if UnixStagingIdentity::from_stat(&metadata)? != self.identity {
            return Err(String::from(
                "staging directory entry was replaced before unlink",
            ));
        }
        // SAFETY: `parent` is a live directory descriptor and `name` is a valid
        // NUL-terminated entry. AT_REMOVEDIR cannot unlink a non-directory.
        if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
            return Err(format!(
                "owned staging directory could not be unlinked: {}",
                std::io::Error::last_os_error()
            ));
        }
        let metadata = self
            .directory
            .metadata()
            .map_err(|error| format!("owned staging directory identity is unavailable: {error}"))?;
        if UnixStagingIdentity::from_metadata(&metadata)? != self.identity {
            return Err(String::from(
                "owned staging directory changed during unlink",
            ));
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn current_path(&self, _original: &Path) -> Result<PathBuf, String> {
        use std::os::fd::AsRawFd as _;

        let fd_path = PathBuf::from(format!("/proc/self/fd/{}", self.directory.as_raw_fd()));
        std::fs::read_link(fd_path)
            .map_err(|error| format!("owned staging directory path is unavailable: {error}"))
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    fn current_path(&self, original: &Path) -> Result<PathBuf, String> {
        Ok(original.to_path_buf())
    }
}

#[cfg(unix)]
impl UnixStagingIdentity {
    fn from_metadata(metadata: &std::fs::Metadata) -> Result<Self, String> {
        use std::os::unix::fs::MetadataExt as _;

        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    #[expect(clippy::unnecessary_cast, reason = "st_dev and st_ino vary by Unix")]
    fn from_stat(metadata: &libc::stat) -> Result<Self, String> {
        Ok(Self {
            device: metadata.st_dev as u64,
            inode: metadata.st_ino as u64,
        })
    }
}

#[cfg(unix)]
fn directory_identity(directory: &std::fs::File) -> Result<UnixStagingIdentity, String> {
    let metadata = directory
        .metadata()
        .map_err(|error| format!("staging directory identity is unavailable: {error}"))?;
    UnixStagingIdentity::from_metadata(&metadata)
}

#[cfg(unix)]
fn remove_unix_directory_contents(directory_fd: i32) -> Result<(), String> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};

    // SAFETY: directory_fd is live; fcntl only inspects it and returns a new
    // descriptor or -1 without dereferencing Rust memory.
    let duplicate = unsafe { libc::fcntl(directory_fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err(format!(
            "staging directory descriptor could not be duplicated: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `duplicate` is a newly owned descriptor, and fdopendir takes
    // ownership on success. On failure this branch closes it before returning.
    let iterator = unsafe { libc::fdopendir(duplicate) };
    if iterator.is_null() {
        let error = std::io::Error::last_os_error();
        // SAFETY: `duplicate` was not transferred because fdopendir failed.
        unsafe { libc::close(duplicate) };
        return Err(format!("staging directory could not be read: {error}"));
    }
    struct Directory(*mut libc::DIR);
    impl Drop for Directory {
        fn drop(&mut self) {
            // SAFETY: the pointer came from fdopendir and is closed exactly once.
            unsafe { libc::closedir(self.0) };
        }
    }
    let iterator = Directory(iterator);
    // SAFETY: libc::stat is a plain output structure and its all-zero value is
    // a valid writable buffer before fstat fills it.
    let mut root_metadata = unsafe { std::mem::zeroed::<libc::stat>() };
    // SAFETY: directory_fd is live and root_metadata is writable.
    if unsafe { libc::fstat(directory_fd, &mut root_metadata) } != 0 {
        return Err(format!(
            "staging directory metadata is unavailable: {}",
            std::io::Error::last_os_error()
        ));
    }
    let root_device = root_metadata.st_dev;
    loop {
        errno_clear();
        // SAFETY: iterator.0 is a live DIR owned by the guard, and readdir
        // returns either null or a pointer valid until the next call.
        let entry = unsafe { libc::readdir(iterator.0) };
        if entry.is_null() {
            let error = errno_value();
            if error != 0 {
                return Err(format!(
                    "staging directory could not be read: {}",
                    std::io::Error::from_raw_os_error(error)
                ));
            }
            return Ok(());
        }
        // SAFETY: entry is non-null and remains valid until the next readdir;
        // d_name is a NUL-terminated platform byte string.
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        let name = std::ffi::CString::new(name)
            .map_err(|_| String::from("staging entry name is invalid"))?;
        // SAFETY: libc::stat is a plain C data structure and an all-zero value
        // is a valid writable output buffer before fstatat fills it.
        let mut metadata = unsafe { std::mem::zeroed::<libc::stat>() };
        // SAFETY: `directory_fd` is live, `name` is NUL-terminated, and metadata
        // is writable. AT_SYMLINK_NOFOLLOW prevents crossing a replacement link.
        if unsafe {
            libc::fstatat(
                directory_fd,
                name.as_ptr(),
                &mut metadata,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(format!(
                "staging entry metadata is unavailable: {}",
                std::io::Error::last_os_error()
            ));
        }
        if metadata.st_dev != root_device {
            return Err(String::from("staging entry crosses a filesystem boundary"));
        }
        if metadata.st_mode & libc::S_IFMT == libc::S_IFDIR {
            // SAFETY: the entry was just identified as a directory without
            // following links. The returned descriptor transfers ownership.
            let child = unsafe {
                libc::openat(
                    directory_fd,
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                )
            };
            if child < 0 {
                return Err(format!(
                    "staging child directory could not be opened: {}",
                    std::io::Error::last_os_error()
                ));
            }
            // SAFETY: `child` is a new owned descriptor.
            let child = unsafe { std::os::fd::OwnedFd::from_raw_fd(child) };
            remove_unix_directory_contents(child.as_raw_fd())?;
            // SAFETY: child and name remain live until the directory is empty.
            if unsafe { libc::unlinkat(directory_fd, name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
                return Err(format!(
                    "staging child directory could not be removed: {}",
                    std::io::Error::last_os_error()
                ));
            }
        } else if metadata.st_mode & libc::S_IFMT == libc::S_IFREG
            || metadata.st_mode & libc::S_IFMT == libc::S_IFLNK
        {
            // SAFETY: the entry was identified without following a link and
            // name is NUL-terminated. A replacement can only remove the current
            // entry inside the owned directory, never the directory itself.
            if unsafe { libc::unlinkat(directory_fd, name.as_ptr(), 0) } != 0 {
                return Err(format!(
                    "staging entry could not be removed: {}",
                    std::io::Error::last_os_error()
                ));
            }
        } else {
            return Err(String::from("staging tree contains a special file"));
        }
    }
}

#[cfg(unix)]
fn errno_clear() {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        // SAFETY: __errno_location returns the valid thread-local errno slot.
        unsafe {
            *libc::__errno_location() = 0;
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        // SAFETY: __error returns the valid thread-local errno slot.
        unsafe {
            *libc::__error() = 0;
        }
    }
}

#[cfg(unix)]
fn errno_value() -> i32 {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        // SAFETY: __errno_location returns the valid thread-local errno slot.
        unsafe { *libc::__errno_location() }
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        // SAFETY: __error returns the valid thread-local errno slot.
        unsafe { *libc::__error() }
    }
}

#[cfg(windows)]
struct StagingObject {
    handle: isize,
    identity: WindowsStagingIdentity,
}

#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct WindowsStagingIdentity {
    volume: u32,
    index: u64,
}

#[cfg(windows)]
impl StagingObject {
    fn open(path: &Path) -> Result<Self, String> {
        use std::os::windows::ffi::OsStrExt as _;
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, CreateFileW, DELETE, FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE, FILE_TRAVERSE, GetFileInformationByHandle, OPEN_EXISTING,
            SYNCHRONIZE,
        };

        let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
        wide.push(0);
        // SAFETY: `wide` is NUL-terminated and the flags permit opening a
        // directory while retaining delete sharing. The handle is owned below.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_READ_ATTRIBUTES | DELETE | FILE_LIST_DIRECTORY | FILE_TRAVERSE | SYNCHRONIZE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(format!(
                "staging directory could not be opened: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: BY_HANDLE_FILE_INFORMATION is a plain C output structure and
        // its all-zero value is valid before GetFileInformationByHandle fills it.
        let mut information = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
        // SAFETY: handle is owned and information is writable for this call.
        let queried = unsafe { GetFileInformationByHandle(handle, &mut information) };
        if queried == 0 {
            let error = std::io::Error::last_os_error();
            // SAFETY: handle is owned and is not used after this close.
            let _ = unsafe { CloseHandle(handle) };
            return Err(format!(
                "staging directory identity is unavailable: {error}"
            ));
        }
        if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        {
            // SAFETY: handle is owned and is not used after this close.
            let _ = unsafe { CloseHandle(handle) };
            return Err(String::from(
                "staging object is not a real directory handle",
            ));
        }
        Ok(Self {
            handle: handle as isize,
            identity: WindowsStagingIdentity {
                volume: information.dwVolumeSerialNumber,
                index: (u64::from(information.nFileIndexHigh) << 32)
                    | u64::from(information.nFileIndexLow),
            },
        })
    }

    fn set_private_directory_permissions(&self) -> Result<(), String> {
        Ok(())
    }

    fn validate_current_path(
        &self,
        workspace_root: &WorkspaceRoot,
        _original: &Path,
    ) -> Result<(), String> {
        let current = self.current_path()?;
        workspace_root.validate_staging_directory(&current)?;
        workspace_root.validate_staging_tree(&current)?;
        Ok(())
    }

    fn remove_contents(&self) -> Result<(), String> {
        use windows_sys::Win32::Foundation::HANDLE;

        remove_windows_directory_contents(self.handle as HANDLE, 0)
    }

    fn remove_directory_entry(&self, _original_path: &Path) -> Result<(), String> {
        use windows_sys::Win32::Foundation::HANDLE;

        mark_windows_handle_delete(self.handle as HANDLE)
    }

    fn current_path(&self) -> Result<PathBuf, String> {
        use std::os::windows::ffi::OsStringExt as _;
        use windows_sys::Win32::Foundation::{HANDLE, MAX_PATH};
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
        };

        let mut capacity = MAX_PATH as usize;
        let mut buffer = vec![0u16; capacity];
        loop {
            // SAFETY: handle is owned, buffer is writable, and capacity is its
            // exact element count. The returned length is used only as a size.
            let length = unsafe {
                GetFinalPathNameByHandleW(
                    self.handle as HANDLE,
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
                )
            };
            if length == 0 {
                return Err(format!(
                    "owned staging directory path is unavailable: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if length < buffer.len() as u32 {
                buffer.truncate(length as usize);
                let wide = std::ffi::OsString::from_wide(&buffer);
                return Ok(PathBuf::from(wide));
            }
            capacity = length as usize + 1;
            buffer.resize(capacity, 0);
        }
    }
}

#[cfg(windows)]
impl Drop for StagingObject {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};

        // SAFETY: the handle is owned by this value and is closed exactly once.
        unsafe { CloseHandle(self.handle as HANDLE) };
    }
}

#[cfg(windows)]
const MAX_WINDOWS_STAGING_DEPTH: usize = 64;
#[cfg(windows)]
const MAX_WINDOWS_ENUMERATION_RESTARTS: usize = 8;
#[cfg(windows)]
const MAX_WINDOWS_DIRECTORY_BUFFER_BYTES: usize = 1024 * 1024;

#[cfg(windows)]
struct WindowsDirectoryEntry {
    name: Vec<u16>,
    attributes: u32,
}

#[cfg(windows)]
struct WindowsOwnedHandle {
    handle: isize,
}

#[cfg(windows)]
impl Drop for WindowsOwnedHandle {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};

        // SAFETY: the handle is owned by this value and is closed exactly once.
        unsafe { CloseHandle(self.handle as HANDLE) };
    }
}

#[cfg(windows)]
fn remove_windows_directory_contents(root: HANDLE, depth: usize) -> Result<(), String> {
    if depth > MAX_WINDOWS_STAGING_DEPTH {
        return Err(String::from("staging tree exceeds the supported depth"));
    }
    let mut restart = true;
    let mut attempts = 0usize;
    loop {
        attempts += 1;
        if attempts > MAX_WINDOWS_ENUMERATION_RESTARTS {
            return Err(String::from(
                "staging directory did not quiesce within the cleanup retry bound",
            ));
        }
        let (entries, more_data) = read_windows_directory_page(root, restart)?;
        let mut descended = false;
        for entry in entries {
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
            };

            if entry.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(String::from("staging tree contains a reparse point"));
            }
            let is_directory = entry.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
            let Some(child) = open_windows_child(root, &entry.name, is_directory)? else {
                restart = true;
                descended = true;
                break;
            };
            if is_directory {
                remove_windows_directory_contents(child.handle as HANDLE, depth + 1)?;
            }
            mark_windows_handle_delete(child.handle as HANDLE)?;
            if is_directory {
                restart = true;
                descended = true;
                break;
            }
        }
        if descended {
            continue;
        }
        if more_data {
            restart = false;
            continue;
        }
        let (remaining, _) = read_windows_directory_page(root, true)?;
        if remaining.is_empty() {
            return Ok(());
        }
        restart = true;
    }
}

#[cfg(windows)]
fn read_windows_directory_page(
    directory: HANDLE,
    restart: bool,
) -> Result<(Vec<WindowsDirectoryEntry>, bool), String> {
    use windows_sys::Win32::Foundation::ERROR_NO_MORE_FILES;
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo, GetFileInformationByHandleEx,
    };

    let mut words = vec![0u64; 1024];
    loop {
        let byte_length = words.len() * size_of::<u64>();
        let class = if restart {
            FileIdBothDirectoryRestartInfo
        } else {
            FileIdBothDirectoryInfo
        };
        // SAFETY: directory is a live directory handle, words is aligned and
        // writable for byte_length bytes, and class is a supported enum class.
        let result = unsafe {
            GetFileInformationByHandleEx(
                directory,
                class,
                words.as_mut_ptr().cast(),
                byte_length as u32,
            )
        };
        if result == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                return Ok((Vec::new(), false));
            }
            if error.raw_os_error() == Some(234) {
                if byte_length >= MAX_WINDOWS_DIRECTORY_BUFFER_BYTES {
                    return Err(String::from(
                        "staging directory enumeration exceeded its size bound",
                    ));
                }
                words.resize(words.len() * 2, 0);
                continue;
            }
            return Err(format!(
                "staging directory could not be enumerated: {error}"
            ));
        }
        return parse_windows_directory_page(&words, byte_length);
    }
}

#[cfg(windows)]
fn parse_windows_directory_page(
    words: &[u64],
    byte_length: usize,
) -> Result<(Vec<WindowsDirectoryEntry>, bool), String> {
    use windows_sys::Win32::Storage::FileSystem::FILE_ID_BOTH_DIR_INFO;

    let base = words.as_ptr().cast::<u8>();
    let mut offset = 0usize;
    let mut entries = Vec::new();
    loop {
        let header_bytes = size_of::<FILE_ID_BOTH_DIR_INFO>();
        if offset > byte_length || byte_length - offset < header_bytes {
            return Err(String::from("staging directory enumeration was malformed"));
        }
        // SAFETY: offset is within the initialized API buffer and the record
        // header fits; reads below stay within the validated name length.
        let info = unsafe { base.add(offset).cast::<FILE_ID_BOTH_DIR_INFO>() };
        // SAFETY: offset is bounds-checked and the API initialized each fixed
        // field plus FileNameLength bytes of the trailing name.
        let (next, name_length, attributes, name_start) = unsafe {
            (
                std::ptr::addr_of!((*info).NextEntryOffset).read_unaligned() as usize,
                std::ptr::addr_of!((*info).FileNameLength).read_unaligned() as usize,
                std::ptr::addr_of!((*info).FileAttributes).read_unaligned(),
                std::ptr::addr_of!((*info).FileName).cast::<u8>() as usize - base as usize,
            )
        };
        if name_length % size_of::<u16>() != 0
            || name_start > byte_length
            || name_length > byte_length - name_start
        {
            return Err(String::from("staging directory entry name was malformed"));
        }
        let name = (0..name_length / size_of::<u16>())
            .map(|index| {
                // SAFETY: index is below the validated FileNameLength and the
                // API initialized that many UTF-16 code units.
                unsafe {
                    std::ptr::addr_of!((*info).FileName)
                        .cast::<u16>()
                        .add(index)
                        .read_unaligned()
                }
            })
            .collect::<Vec<_>>();
        let dot = u16::from(b'.');
        if name.as_slice() != [dot] && name.as_slice() != [dot, dot] {
            entries.push(WindowsDirectoryEntry { name, attributes });
        }
        if next == 0 {
            return Ok((entries, false));
        }
        if next < header_bytes || next % size_of::<u64>() != 0 || next > byte_length - offset {
            return Err(String::from("staging directory entry offset was malformed"));
        }
        offset += next;
    }
}

#[cfg(windows)]
fn open_windows_child(
    parent: HANDLE,
    name: &[u16],
    directory: bool,
) -> Result<Option<WindowsOwnedHandle>, String> {
    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN_REPARSE_POINT,
        FILE_SYNCHRONOUS_IO_NONALERT, NtOpenFile,
    };
    use windows_sys::Win32::Foundation::{OBJ_DONT_REPARSE, RtlNtStatusToDosError, UNICODE_STRING};
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FILE_TRAVERSE, SYNCHRONIZE,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let byte_length = name
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| String::from("staging entry name was too long"))?;
    let mut buffer = name.to_vec();
    buffer.push(0);
    let maximum_length = u16::try_from(byte_length as usize + size_of::<u16>())
        .map_err(|_| String::from("staging entry name was too long"))?;
    let unicode_name = UNICODE_STRING {
        Length: byte_length,
        MaximumLength: maximum_length,
        Buffer: buffer.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent,
        ObjectName: &raw const unicode_name,
        Attributes: OBJ_DONT_REPARSE,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let access = if directory {
        DELETE | FILE_LIST_DIRECTORY | FILE_TRAVERSE | SYNCHRONIZE
    } else {
        DELETE
    };
    let options = FILE_OPEN_REPARSE_POINT
        | if directory {
            FILE_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT
        } else {
            FILE_NON_DIRECTORY_FILE
        };
    let mut handle = std::ptr::null_mut();
    let mut status_block = IO_STATUS_BLOCK::default();
    // SAFETY: parent is live, unicode_name and buffer outlive the call,
    // attributes names a single child relative to parent, and status_block is writable.
    let status = unsafe {
        NtOpenFile(
            &mut handle,
            access,
            &raw const attributes,
            &mut status_block,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            options,
        )
    };
    if status >= 0 {
        return Ok(Some(WindowsOwnedHandle {
            handle: handle as isize,
        }));
    }
    // SAFETY: status is a valid NTSTATUS returned by NtOpenFile.
    let error = unsafe { RtlNtStatusToDosError(status) };
    if matches!(error, 2 | 3 | 303) {
        return Ok(None);
    }
    Err(format!(
        "staging child could not be opened by owned directory handle: {}",
        std::io::Error::from_raw_os_error(error as i32)
    ))
}

#[cfg(windows)]
fn mark_windows_handle_delete(handle: HANDLE) -> Result<(), String> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO,
        FILE_DISPOSITION_INFO_EX, FileDispositionInfo, FileDispositionInfoEx,
        SetFileInformationByHandle,
    };

    let information = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    };
    // SAFETY: handle has delete access and information is a correctly sized
    // FileDispositionInfoEx value for the duration of the call.
    let deleted = unsafe {
        SetFileInformationByHandle(
            handle,
            FileDispositionInfoEx,
            std::ptr::from_ref(&information).cast(),
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    };
    if deleted != 0 {
        return Ok(());
    }
    let first_error = std::io::Error::last_os_error();
    if !matches!(first_error.raw_os_error(), Some(1 | 50 | 87)) {
        return Err(format!(
            "owned staging object could not be deleted by handle: {first_error}"
        ));
    }
    let information = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: handle has delete access and information is a correctly sized
    // classic disposition value.
    if unsafe {
        SetFileInformationByHandle(
            handle,
            FileDispositionInfo,
            std::ptr::from_ref(&information).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(format!(
            "owned staging object could not be deleted by handle: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}
