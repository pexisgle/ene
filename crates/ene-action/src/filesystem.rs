use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use tempfile::NamedTempFile;
use thiserror::Error;

use crate::attempt::{ActionCertainty, EffectGrounds, OperationKind, RealTargetRef};

const STAGING_OWNERSHIP_MARKER: &str = ".ene-action-staging-owner";

#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceEffectStagingPause {
    pub entered: PathBuf,
    pub release: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceEffectOptions {
    pub staging_directory: Option<PathBuf>,
    pub staging_ownership_token: Option<String>,
    pub staging_identity: Option<String>,
    #[cfg(any(test, feature = "test-support"))]
    pub pause_after_staging: Option<WorkspaceEffectStagingPause>,
}

static STAGING_TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

struct BoundStagingTemp {
    staging: fs::File,
    file: fs::File,
    name: std::ffi::OsString,
    published: bool,
}

impl BoundStagingTemp {
    fn create(path: &Path, ownership_token: &str, identity: &str) -> Option<Self> {
        let staging = open_bound_directory(path)?;
        if staging_directory_identity_token(&staging).as_deref() != Some(identity) {
            return None;
        }
        if read_bound_staging_marker(&staging).as_deref() != Some(ownership_token) {
            return None;
        }
        for _ in 0..32 {
            let sequence = STAGING_TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let name = std::ffi::OsString::from(format!(
                ".ene-action-{}-{sequence}.tmp",
                std::process::id()
            ));
            match create_bound_temp(&staging, &name) {
                Some(file) => {
                    return Some(Self {
                        staging,
                        file,
                        name,
                        published: false,
                    });
                }
                None if bound_entry_exists(&staging, &name) => {}
                None => return None,
            }
        }
        None
    }

    fn file_mut(&mut self) -> &mut fs::File {
        &mut self.file
    }

    fn publish(&mut self, destination: &Path, replace: bool) -> Result<(), ()> {
        publish_bound_temp(&self.staging, &self.file, &self.name, destination, replace)?;
        self.published = true;
        Ok(())
    }
}

impl Drop for BoundStagingTemp {
    fn drop(&mut self) {
        if !self.published {
            discard_bound_temp(&self.staging, &self.file, &self.name);
        }
    }
}

#[cfg(unix)]
fn open_bound_directory(path: &Path) -> Option<fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .ok()
}

#[cfg(windows)]
fn open_bound_directory(path: &Path) -> Option<fs::File> {
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::{FromRawHandle as _, RawHandle};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ADD_FILE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_DELETE_CHILD, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_TRAVERSE, OPEN_EXISTING, SYNCHRONIZE,
    };

    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    wide.push(0);
    // SAFETY: wide is NUL-terminated; a successful handle is transferred to File.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES
                | FILE_LIST_DIRECTORY
                | FILE_ADD_FILE
                | FILE_DELETE_CHILD
                | FILE_TRAVERSE
                | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    // SAFETY: CreateFileW returned a newly owned handle.
    let file = unsafe { fs::File::from_raw_handle(handle as RawHandle) };
    let metadata = file.metadata().ok()?;
    use std::os::windows::fs::MetadataExt as _;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY == 0
    {
        return None;
    }
    Some(file)
}

#[cfg(not(any(unix, windows)))]
fn open_bound_directory(_path: &Path) -> Option<fs::File> {
    None
}

#[cfg(unix)]
fn staging_directory_identity_token(directory: &fs::File) -> Option<String> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = directory.metadata().ok()?;
    Some(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn staging_directory_identity_token(directory: &fs::File) -> Option<String> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    // SAFETY: information is a writable output buffer and directory owns the handle.
    let mut information = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    // SAFETY: the handle remains live throughout the call.
    if unsafe { GetFileInformationByHandle(directory.as_raw_handle() as HANDLE, &mut information) }
        == 0
    {
        return None;
    }
    Some(format!(
        "windows:{}:{}",
        information.dwVolumeSerialNumber,
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow)
    ))
}

#[cfg(not(any(unix, windows)))]
fn staging_directory_identity_token(_directory: &fs::File) -> Option<String> {
    None
}

#[cfg(unix)]
fn read_bound_staging_marker(directory: &fs::File) -> Option<String> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};

    let name = std::ffi::CString::new(STAGING_OWNERSHIP_MARKER.as_bytes()).ok()?;
    // SAFETY: directory is a live directory fd and name is NUL-terminated.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return None;
    }
    // SAFETY: openat returned a newly owned descriptor.
    let mut file = unsafe { fs::File::from_raw_fd(fd) };
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut token = String::new();
    file.read_to_string(&mut token).ok()?;
    (!token.is_empty()).then_some(token)
}

#[cfg(windows)]
fn read_bound_staging_marker(directory: &fs::File) -> Option<String> {
    let mut file = open_windows_relative_file(
        directory,
        std::ffi::OsStr::new(STAGING_OWNERSHIP_MARKER),
        false,
    )?;
    let mut token = String::new();
    file.read_to_string(&mut token).ok()?;
    (!token.is_empty()).then_some(token)
}

#[cfg(not(any(unix, windows)))]
fn read_bound_staging_marker(_directory: &fs::File) -> Option<String> {
    None
}

#[cfg(unix)]
fn create_bound_temp(directory: &fs::File, name: &std::ffi::OsStr) -> Option<fs::File> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::os::unix::ffi::OsStrExt as _;

    let name = std::ffi::CString::new(name.as_bytes()).ok()?;
    // SAFETY: directory is live and name is a NUL-terminated relative filename.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    (fd >= 0).then(|| {
        // SAFETY: a non-negative openat result is a newly owned descriptor.
        unsafe { fs::File::from_raw_fd(fd) }
    })
}

#[cfg(windows)]
fn create_bound_temp(directory: &fs::File, name: &std::ffi::OsStr) -> Option<fs::File> {
    open_windows_relative_file(directory, name, true)
}

#[cfg(not(any(unix, windows)))]
fn create_bound_temp(_directory: &fs::File, _name: &std::ffi::OsStr) -> Option<fs::File> {
    None
}

#[cfg(unix)]
fn bound_entry_exists(directory: &fs::File, name: &std::ffi::OsStr) -> bool {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;

    let Ok(name) = std::ffi::CString::new(name.as_bytes()) else {
        return false;
    };
    // SAFETY: metadata is a writable C struct, directory is live, name is valid.
    let mut metadata = unsafe { std::mem::zeroed::<libc::stat>() };
    // SAFETY: directory is a live directory fd, name is NUL-terminated, and
    // metadata is writable for the duration of the call.
    (unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            &mut metadata,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    }) == 0
}

#[cfg(windows)]
fn bound_entry_exists(directory: &fs::File, name: &std::ffi::OsStr) -> bool {
    open_windows_relative_file(directory, name, false).is_some()
}

#[cfg(not(any(unix, windows)))]
fn bound_entry_exists(_directory: &fs::File, _name: &std::ffi::OsStr) -> bool {
    false
}

#[cfg(unix)]
fn discard_bound_temp(directory: &fs::File, _file: &fs::File, name: &std::ffi::OsStr) {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;

    if let Ok(name) = std::ffi::CString::new(name.as_bytes()) {
        // SAFETY: directory is live and name is a valid relative filename.
        let _ = unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) };
    }
}

#[cfg(windows)]
fn discard_bound_temp(_directory: &fs::File, file: &fs::File, _name: &std::ffi::OsStr) {
    let _ = mark_windows_file_delete(file);
}

#[cfg(not(any(unix, windows)))]
fn discard_bound_temp(_directory: &fs::File, _file: &fs::File, _name: &std::ffi::OsStr) {}

#[cfg(unix)]
fn publish_bound_temp(
    staging: &fs::File,
    _file: &fs::File,
    name: &std::ffi::OsStr,
    destination: &Path,
    replace: bool,
) -> Result<(), ()> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;

    let parent = open_bound_directory(destination.parent().ok_or(())?).ok_or(())?;
    let source = std::ffi::CString::new(name.as_bytes()).map_err(|_| ())?;
    let target =
        std::ffi::CString::new(destination.file_name().ok_or(())?.as_bytes()).map_err(|_| ())?;
    if replace {
        // SAFETY: both directory fds are live and both names are NUL-terminated.
        (unsafe {
            libc::renameat(
                staging.as_raw_fd(),
                source.as_ptr(),
                parent.as_raw_fd(),
                target.as_ptr(),
            )
        } == 0)
            .then_some(())
            .ok_or(())
    } else {
        // SAFETY: both directory fds are live and both names are NUL-terminated.
        if unsafe {
            libc::linkat(
                staging.as_raw_fd(),
                source.as_ptr(),
                parent.as_raw_fd(),
                target.as_ptr(),
                0,
            )
        } != 0
        {
            return Err(());
        }
        // The target hard link is already published. A leftover staging link
        // remains inside the owned obligation and will be removed by cleanup.
        // SAFETY: staging is live and source is a valid NUL-terminated filename.
        let _ = unsafe { libc::unlinkat(staging.as_raw_fd(), source.as_ptr(), 0) };
        Ok(())
    }
}

#[cfg(windows)]
fn publish_bound_temp(
    _staging: &fs::File,
    file: &fs::File,
    _name: &std::ffi::OsStr,
    destination: &Path,
    replace: bool,
) -> Result<(), ()> {
    rename_windows_file_by_handle(file, destination, replace)
}

#[cfg(not(any(unix, windows)))]
fn publish_bound_temp(
    _staging: &fs::File,
    _file: &fs::File,
    _name: &std::ffi::OsStr,
    _destination: &Path,
    _replace: bool,
) -> Result<(), ()> {
    Err(())
}

#[cfg(windows)]
fn open_windows_relative_file(
    directory: &fs::File,
    name: &std::ffi::OsStr,
    create: bool,
) -> Option<fs::File> {
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, RawHandle};
    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_OPEN_REPARSE_POINT,
        FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile, NtOpenFile,
    };
    use windows_sys::Win32::Foundation::{HANDLE, OBJ_DONT_REPARSE, UNICODE_STRING};
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, SYNCHRONIZE,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let mut buffer = name.encode_wide().collect::<Vec<_>>();
    let byte_length = buffer.len().checked_mul(size_of::<u16>())?;
    let length = u16::try_from(byte_length).ok()?;
    buffer.push(0);
    let maximum_length = u16::try_from(byte_length + size_of::<u16>()).ok()?;
    let unicode = UNICODE_STRING {
        Length: length,
        MaximumLength: maximum_length,
        Buffer: buffer.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: directory.as_raw_handle() as HANDLE,
        ObjectName: &raw const unicode,
        Attributes: OBJ_DONT_REPARSE,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut handle = std::ptr::null_mut();
    let mut status = IO_STATUS_BLOCK::default();
    let options = FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT;
    // SAFETY: all pointer-backed structs and the directory handle remain live for the call.
    let result = unsafe {
        if create {
            NtCreateFile(
                &mut handle,
                FILE_GENERIC_WRITE | DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                &raw const attributes,
                &mut status,
                std::ptr::null(),
                FILE_ATTRIBUTE_NORMAL,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                FILE_CREATE,
                options,
                std::ptr::null(),
                0,
            )
        } else {
            NtOpenFile(
                &mut handle,
                FILE_GENERIC_READ,
                &raw const attributes,
                &mut status,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                options,
            )
        }
    };
    if result < 0 {
        return None;
    }
    // SAFETY: the successful NT call returned a newly owned handle.
    let file = unsafe { fs::File::from_raw_handle(handle as RawHandle) };
    let metadata = file.metadata().ok()?;
    use std::os::windows::fs::MetadataExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    };
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0
    {
        return None;
    }
    Some(file)
}

#[cfg(windows)]
fn mark_windows_file_delete(file: &fs::File) -> Result<(), ()> {
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO,
        FILE_DISPOSITION_INFO_EX, FileDispositionInfo, FileDispositionInfoEx,
        SetFileInformationByHandle,
    };

    let information = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    };
    // SAFETY: file owns a live DELETE-capable handle and information is correctly sized.
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as HANDLE,
            FileDispositionInfoEx,
            std::ptr::from_ref(&information).cast(),
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    } != 0
    {
        return Ok(());
    }
    let classic = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: same handle and correctly sized fallback structure.
    (unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as HANDLE,
            FileDispositionInfo,
            std::ptr::from_ref(&classic).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } != 0)
        .then_some(())
        .ok_or(())
}

#[cfg(windows)]
fn rename_windows_file_by_handle(
    file: &fs::File,
    destination: &Path,
    replace: bool,
) -> Result<(), ()> {
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_RENAME_INFO, FileRenameInfo, SetFileInformationByHandle,
    };

    let parent = open_bound_directory(destination.parent().ok_or(())?).ok_or(())?;
    let name = destination
        .file_name()
        .ok_or(())?
        .encode_wide()
        .collect::<Vec<_>>();
    let name_bytes = name.len().checked_mul(size_of::<u16>()).ok_or(())?;
    // FILE_RENAME_INFO has a trailing FileName[1], but Windows expects the
    // full fixed structure size plus FileNameLength bytes in the supplied buffer.
    // Over-allocating the placeholder WCHAR is intentional and matches the Win32
    // variable-length structure convention.
    let total_bytes = size_of::<FILE_RENAME_INFO>()
        .checked_add(name_bytes)
        .ok_or(())?;
    let mut buffer = vec![0u64; total_bytes.div_ceil(size_of::<u64>()).max(1)];
    let information = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: the aligned buffer is large enough for the fixed header plus UTF-16 name.
    unsafe {
        (*information).Anonymous.ReplaceIfExists = replace;
        (*information).RootDirectory = parent.as_raw_handle() as HANDLE;
        (*information).FileNameLength = u32::try_from(name_bytes).map_err(|_| ())?;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*information).FileName).cast::<u16>(),
            name.len(),
        );
    }
    // SAFETY: file was opened with DELETE access and all backing buffers/handles stay live.
    (unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as HANDLE,
            FileRenameInfo,
            information.cast(),
            u32::try_from(total_bytes).map_err(|_| ())?,
        )
    } != 0)
        .then_some(())
        .ok_or(())
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WorkspaceRootError {
    #[error("workspace folder is unavailable")]
    Unavailable,
    #[error("workspace folder is not a directory")]
    NotADirectory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntry {
    pub name: String,
    pub kind: ListEntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListEntryKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutput {
    Bytes(Vec<u8>),
    Listing(Vec<ListEntry>),
    Created { target: RealTargetRef },
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TargetRejection {
    #[error("requested path is malformed")]
    MalformedPath,
    #[error("requested path escapes the workspace")]
    OutsideWorkspace,
    #[error("requested target crosses the workspace filesystem boundary")]
    CrossFilesystem,
    #[error("requested target does not exist")]
    MissingTarget,
    #[error("requested target parent does not exist")]
    MissingParent,
    #[error("requested target is not a regular file")]
    NotAFile,
    #[error("requested target is not a directory")]
    NotADirectory,
    #[error("requested target already exists")]
    AlreadyExists,
    #[error("requested target is unavailable")]
    TargetUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRoot {
    root: PathBuf,
}

impl WorkspaceRoot {
    pub fn open(folder: &str) -> Result<Self, WorkspaceRootError> {
        let root = fs::canonicalize(folder).map_err(|_| WorkspaceRootError::Unavailable)?;
        if !root.is_dir() {
            return Err(WorkspaceRootError::NotADirectory);
        }
        Ok(Self { root })
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.root
    }

    pub fn resolve(
        &self,
        requested: &str,
        operation: OperationKind,
    ) -> Result<RealTargetRef, TargetRejection> {
        let names =
            if operation == OperationKind::List && (requested.is_empty() || requested == ".") {
                Vec::new()
            } else {
                requested_components(requested).ok_or(TargetRejection::MalformedPath)?
            };
        match operation {
            OperationKind::List | OperationKind::Read | OperationKind::Edit => {
                let mut joined = self.root.clone();
                for name in &names {
                    joined.push(name);
                }
                let canonical = fs::canonicalize(&joined).map_err(|error| map_io_error(&error))?;
                if !canonical.starts_with(&self.root) {
                    return Err(TargetRejection::OutsideWorkspace);
                }
                let metadata = fs::metadata(&canonical).map_err(|error| map_io_error(&error))?;
                if !self.boundary_holds(&canonical, &metadata) {
                    return Err(TargetRejection::CrossFilesystem);
                }
                match operation {
                    OperationKind::List => {
                        if !metadata.is_dir() {
                            return Err(TargetRejection::NotADirectory);
                        }
                    }
                    _ => {
                        if !metadata.is_file() {
                            return Err(TargetRejection::NotAFile);
                        }
                    }
                }
                Ok(canonical_target(canonical)?)
            }
            OperationKind::Create => {
                let Some((file_name, parent_names)) = names.split_last() else {
                    return Err(TargetRejection::MalformedPath);
                };
                let mut canonical_parent = self.root.clone();
                for name in parent_names {
                    canonical_parent.push(name);
                    let canonical = fs::canonicalize(&canonical_parent).map_err(|error| {
                        if error.kind() == std::io::ErrorKind::NotFound {
                            TargetRejection::MissingParent
                        } else {
                            TargetRejection::TargetUnavailable
                        }
                    })?;
                    if !canonical.starts_with(&self.root) {
                        return Err(TargetRejection::OutsideWorkspace);
                    }
                    if !canonical.is_dir() {
                        return Err(TargetRejection::MissingParent);
                    }
                    let metadata =
                        fs::metadata(&canonical).map_err(|error| map_io_error(&error))?;
                    if !self.boundary_holds(&canonical, &metadata) {
                        return Err(TargetRejection::CrossFilesystem);
                    }
                    canonical_parent = canonical;
                }
                let destination = canonical_parent.join(file_name);
                match fs::symlink_metadata(&destination) {
                    Ok(_) => return Err(TargetRejection::AlreadyExists),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(map_io_error(&error)),
                }
                Ok(canonical_target(destination)?)
            }
        }
    }

    pub fn validate_staging_directory(&self, path: &Path) -> Result<(), String> {
        if !path.is_absolute() || !path.starts_with(&self.root) {
            return Err(String::from("staging path is outside the workspace"));
        }
        if staging_path_has_reparse(&self.root, path) {
            return Err(String::from("staging path contains a reparse point"));
        }
        let canonical = fs::canonicalize(path)
            .map_err(|error| format!("staging directory is unavailable: {error}"))?;
        if canonical != path || !canonical.starts_with(&self.root) {
            return Err(String::from(
                "staging path is not the expected workspace path",
            ));
        }
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| format!("staging directory metadata is unavailable: {error}"))?;
        if metadata_is_reparse(&metadata) || !metadata.is_dir() {
            return Err(String::from("staging path is not a real directory"));
        }
        let target_metadata = fs::metadata(&canonical)
            .map_err(|error| format!("staging directory target is unavailable: {error}"))?;
        if !self.boundary_holds(&canonical, &target_metadata) {
            return Err(String::from(
                "staging path crosses the workspace filesystem boundary",
            ));
        }
        Ok(())
    }

    pub fn validate_staging_tree(&self, path: &Path) -> Result<(), String> {
        self.validate_staging_directory(path)?;
        #[cfg(target_os = "linux")]
        {
            let mounts = linux_mount_points()
                .map_err(|error| format!("staging mount boundary is unavailable: {error}"))?;
            if mounts.iter().any(|mount| mount.starts_with(path)) {
                return Err(String::from("staging tree crosses a Linux mount boundary"));
            }
        }
        #[cfg(windows)]
        if staging_tree_contains_reparse(path)? {
            return Err(String::from("staging tree contains a reparse point"));
        }
        Ok(())
    }

    #[must_use]
    pub(crate) fn execute(
        &self,
        target: &RealTargetRef,
        operation: OperationKind,
        content: Option<&[u8]>,
    ) -> ObservedEffect {
        self.execute_with_options(
            target,
            operation,
            content,
            &WorkspaceEffectOptions::default(),
        )
    }

    pub(crate) fn execute_with_options(
        &self,
        target: &RealTargetRef,
        operation: OperationKind,
        content: Option<&[u8]>,
        options: &WorkspaceEffectOptions,
    ) -> ObservedEffect {
        match operation {
            OperationKind::List => self.list_directory(target),
            OperationKind::Read => {
                let destination = Path::new(target.as_path());
                if !self.verified_existing_metadata(destination, false) {
                    return refused();
                }
                match fs::read(destination) {
                    Ok(bytes) => confirmed(ActionOutput::Bytes(bytes)),
                    Err(_) => refused(),
                }
            }
            OperationKind::Create => {
                let Some(bytes) = content else {
                    return refused();
                };
                self.write_atomically(target, bytes, false, options)
            }
            OperationKind::Edit => {
                let Some(bytes) = content else {
                    return refused();
                };
                self.write_atomically(target, bytes, true, options)
            }
        }
    }

    fn list_directory(&self, target: &RealTargetRef) -> ObservedEffect {
        let destination = Path::new(target.as_path());
        if !self.verified_existing_metadata(destination, true) {
            return refused();
        }
        let Ok(entries) = fs::read_dir(destination) else {
            return refused();
        };
        let mut listing = Vec::new();
        for entry in entries {
            let Ok(entry) = entry else {
                return refused();
            };
            let Some(kind) = self.listable_child(&entry.path()) else {
                continue;
            };
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            listing.push(ListEntry { name, kind });
        }
        listing.sort_by(|left, right| left.name.cmp(&right.name));
        confirmed(ActionOutput::Listing(listing))
    }

    fn listable_child(&self, path: &Path) -> Option<ListEntryKind> {
        let metadata = fs::symlink_metadata(path).ok()?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return None;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return None;
            }
        }
        let kind = if file_type.is_file() {
            ListEntryKind::File
        } else if file_type.is_dir() {
            ListEntryKind::Directory
        } else {
            return None;
        };
        if !self.boundary_holds(path, &metadata) {
            return None;
        }
        Some(kind)
    }

    fn write_atomically(
        &self,
        target: &RealTargetRef,
        bytes: &[u8],
        replace: bool,
        options: &WorkspaceEffectOptions,
    ) -> ObservedEffect {
        let destination = Path::new(target.as_path());
        if !self.reverifies_at_effect(destination, replace) {
            return refused();
        }
        let Some(parent) = destination.parent() else {
            return refused();
        };
        if let Some(staging_directory) = options.staging_directory.as_deref() {
            let (Some(ownership_token), Some(identity)) = (
                options.staging_ownership_token.as_deref(),
                options.staging_identity.as_deref(),
            ) else {
                return refused();
            };
            let Some(mut temporary) =
                BoundStagingTemp::create(staging_directory, ownership_token, identity)
            else {
                return refused();
            };
            {
                let file = temporary.file_mut();
                if file.write_all(bytes).is_err() || file.sync_all().is_err() {
                    return refused();
                }
            }
            #[cfg(any(test, feature = "test-support"))]
            if let Some(pause) = options.pause_after_staging.as_ref()
                && pause.wait().is_err()
            {
                return refused();
            }
            if temporary.publish(destination, replace).is_err() {
                return refused();
            }
            if temporary.file.sync_all().is_err() {
                return unverified();
            }
            return match fs::read(destination) {
                Ok(read_back) if read_back == bytes => confirmed(if replace {
                    ActionOutput::Updated
                } else {
                    ActionOutput::Created {
                        target: target.clone(),
                    }
                }),
                _ => unverified(),
            };
        }

        let mut temporary = match NamedTempFile::new_in(parent) {
            Ok(temporary) => temporary,
            Err(_) => return refused(),
        };
        {
            let file = temporary.as_file_mut();
            if file.write_all(bytes).is_err() || file.sync_all().is_err() {
                return refused();
            }
        }
        #[cfg(any(test, feature = "test-support"))]
        if let Some(pause) = options.pause_after_staging.as_ref()
            && pause.wait().is_err()
        {
            return refused();
        }
        let persisted = if replace {
            temporary.persist(destination)
        } else {
            temporary.persist_noclobber(destination)
        };
        let persisted = match persisted {
            Ok(file) => file,
            Err(_) => return refused(),
        };
        if persisted.sync_all().is_err() {
            return unverified();
        }
        match fs::read(destination) {
            Ok(read_back) if read_back == bytes => confirmed(if replace {
                ActionOutput::Updated
            } else {
                ActionOutput::Created {
                    target: target.clone(),
                }
            }),
            _ => unverified(),
        }
    }

    fn verified_existing_metadata(&self, destination: &Path, want_directory: bool) -> bool {
        let Ok(canonical) = fs::canonicalize(destination) else {
            return false;
        };
        if canonical != destination || !canonical.starts_with(&self.root) {
            return false;
        }
        let Ok(metadata) = fs::metadata(&canonical) else {
            return false;
        };
        metadata.is_dir() == want_directory && self.boundary_holds(&canonical, &metadata)
    }

    fn reverifies_at_effect(&self, destination: &Path, replace: bool) -> bool {
        if replace {
            return self.verified_existing_metadata(destination, false);
        }
        let Some(parent) = destination.parent() else {
            return false;
        };
        self.verified_existing_metadata(parent, true) && fs::symlink_metadata(destination).is_err()
    }

    #[cfg(unix)]
    fn boundary_holds(&self, target: &Path, metadata: &fs::Metadata) -> bool {
        use std::os::unix::fs::MetadataExt;

        let Ok(root_metadata) = fs::metadata(&self.root) else {
            return false;
        };
        if root_metadata.dev() != metadata.dev() {
            return false;
        }
        #[cfg(target_os = "linux")]
        {
            match linux_mount_points() {
                Ok(mounts) => !crosses_linux_mount(&self.root, target, &mounts),
                Err(_) => false,
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = target;
            true
        }
    }

    #[cfg(windows)]
    fn boundary_holds(&self, target: &Path, _metadata: &fs::Metadata) -> bool {
        match (
            Self::volume_serial_of(&self.root),
            Self::volume_serial_of(target),
        ) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        }
    }

    #[cfg(windows)]
    fn volume_serial_of(path: &Path) -> Option<u32> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::MAX_PATH;
        use windows_sys::Win32::Storage::FileSystem::{GetVolumeInformationW, GetVolumePathNameW};

        let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
        name.push(0);
        let mut mount = vec![0u16; MAX_PATH as usize];
        // SAFETY: `name` is NUL-terminated and `mount` has exactly the passed
        // capacity; both outlive the call and the buffer is only read after
        // the success flag is checked.
        let mapped =
            unsafe { GetVolumePathNameW(name.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) };
        if mapped == 0 {
            return None;
        }
        let mut serial = 0u32;
        // SAFETY: `mount` is NUL-terminated by the successful call above;
        // unneeded out buffers are NULL, which the API permits, and `serial`
        // is a live local exclusively borrowed for the call.
        let read = unsafe {
            GetVolumeInformationW(
                mount.as_ptr(),
                std::ptr::null_mut(),
                0,
                &mut serial,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        };
        if read == 0 {
            return None;
        }
        Some(serial)
    }
}

fn staging_path_has_reparse(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return true;
    };
    let mut current = root.to_path_buf();
    if fs::symlink_metadata(&current)
        .map(|metadata| metadata_is_reparse(&metadata))
        .unwrap_or(true)
    {
        return true;
    }
    for component in relative.components() {
        current.push(component.as_os_str());
        if fs::symlink_metadata(&current)
            .map(|metadata| metadata_is_reparse(&metadata))
            .unwrap_or(true)
        {
            return true;
        }
    }
    false
}

fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
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

#[cfg(test)]
fn staging_identity_token(path: &Path) -> Option<String> {
    let directory = open_bound_directory(path)?;
    staging_directory_identity_token(&directory)
}

#[cfg(windows)]
fn staging_tree_contains_reparse(path: &Path) -> Result<bool, String> {
    let mut pending = vec![path.to_path_buf()];
    while let Some(candidate) = pending.pop() {
        let metadata = fs::symlink_metadata(&candidate)
            .map_err(|error| format!("staging tree metadata is unavailable: {error}"))?;
        if metadata_is_reparse(&metadata) {
            return Ok(true);
        }
        if metadata.is_dir() {
            let entries = fs::read_dir(&candidate)
                .map_err(|error| format!("staging tree is unavailable: {error}"))?;
            for entry in entries {
                pending.push(
                    entry
                        .map_err(|error| format!("staging tree entry is unavailable: {error}"))?
                        .path(),
                );
            }
        }
    }
    Ok(false)
}

#[cfg(target_os = "linux")]
fn linux_mount_points() -> Result<Vec<PathBuf>, std::io::Error> {
    let content = fs::read_to_string("/proc/self/mountinfo")?;
    Ok(content.lines().filter_map(parse_mount_point).collect())
}

#[cfg(target_os = "linux")]
fn parse_mount_point(line: &str) -> Option<PathBuf> {
    let field = line.split_whitespace().nth(4)?;
    if field.is_empty() {
        return None;
    }
    Some(PathBuf::from(decode_mountinfo_escape(field)))
}

#[cfg(target_os = "linux")]
fn decode_mountinfo_escape(field: &str) -> String {
    let mut decoded = field.to_owned();
    for (escape, character) in [
        ("\\040", " "),
        ("\\011", "\t"),
        ("\\012", "\n"),
        ("\\134", "\\"),
    ] {
        decoded = decoded.replace(escape, character);
    }
    decoded
}

#[cfg(target_os = "linux")]
fn crosses_linux_mount(root: &Path, target: &Path, mounts: &[PathBuf]) -> bool {
    mounts
        .iter()
        .any(|mount| mount != root && mount.starts_with(root) && target.starts_with(mount))
}

#[derive(Clone, PartialEq, Eq)]
pub struct ObservedEffect {
    pub certainty: ActionCertainty,
    pub grounds: EffectGrounds,
    pub output: Option<ActionOutput>,
}

impl core::fmt::Debug for ObservedEffect {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ObservedEffect")
            .field("certainty", &self.certainty)
            .field("grounds", &self.grounds)
            .field(
                "output",
                &self.output.as_ref().map(|output| match output {
                    ActionOutput::Bytes(bytes) if bytes.is_empty() => String::from("<0 bytes>"),
                    ActionOutput::Bytes(bytes) => format!("<{} bytes redacted>", bytes.len()),
                    ActionOutput::Listing(entries) => {
                        format!("<{} entries redacted>", entries.len())
                    }
                    ActionOutput::Created { .. } => String::from("<created target redacted>"),
                    ActionOutput::Updated => String::from("<updated marker>"),
                }),
            )
            .finish()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl WorkspaceEffectStagingPause {
    fn wait(&self) -> Result<(), ()> {
        std::fs::write(&self.entered, b"staged").map_err(|_| ())?;
        while !self.release.exists() {
            std::thread::yield_now();
        }
        Ok(())
    }
}

fn refused() -> ObservedEffect {
    ObservedEffect {
        certainty: ActionCertainty::ConfirmedFailure,
        grounds: EffectGrounds::RefusedBeforeEffect,
        output: None,
    }
}

fn confirmed(output: ActionOutput) -> ObservedEffect {
    ObservedEffect {
        certainty: ActionCertainty::ConfirmedSuccess,
        grounds: EffectGrounds::ObservedAtTarget,
        output: Some(output),
    }
}

fn unverified() -> ObservedEffect {
    ObservedEffect {
        certainty: ActionCertainty::Unknown,
        grounds: EffectGrounds::OutcomeUnverified,
        output: None,
    }
}

fn canonical_target(path: PathBuf) -> Result<RealTargetRef, TargetRejection> {
    match path.to_str() {
        Some(text) => Ok(RealTargetRef::from_canonical_path(text.to_owned())),
        None => Err(TargetRejection::TargetUnavailable),
    }
}

fn map_io_error(error: &std::io::Error) -> TargetRejection {
    match error.kind() {
        std::io::ErrorKind::NotFound => TargetRejection::MissingTarget,
        _ => TargetRejection::TargetUnavailable,
    }
}

fn requested_components(requested: &str) -> Option<Vec<String>> {
    let mut names = Vec::new();
    for component in Path::new(requested).components() {
        match component {
            Component::Normal(name) => names.push(name.to_str()?.to_owned()),
            Component::RootDir
            | Component::Prefix(_)
            | Component::ParentDir
            | Component::CurDir => {
                return None;
            }
        }
    }
    if names.is_empty() { None } else { Some(names) }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        ActionOutput, ListEntry, ListEntryKind, TargetRejection, WorkspaceEffectOptions,
        WorkspaceRoot,
    };
    use crate::attempt::OperationKind;

    fn workspace() -> (tempfile::TempDir, WorkspaceRoot) {
        let directory = tempdir().expect("test workspace directory");
        let root = WorkspaceRoot::open(&directory.path().to_string_lossy())
            .expect("an existing directory opens");
        (directory, root)
    }

    #[test]
    fn open_refuses_missing_and_non_directory_folders() {
        assert!(WorkspaceRoot::open("/nonexistent/ene/workspace").is_err());
        let directory = tempdir().expect("temporary directory");
        let file = directory.path().join("a.txt");
        fs::write(&file, b"x").expect("fixture write");
        assert!(
            WorkspaceRoot::open(&file.to_string_lossy()).is_err(),
            "a regular file is not a workspace folder"
        );
    }

    #[test]
    fn traversal_and_absolute_requests_are_malformed() {
        let (_directory, root) = workspace();
        for requested in [
            "../escape.txt",
            "a/../../escape.txt",
            "/etc/passwd",
            "./a.txt",
            ".",
            "..",
        ] {
            assert_eq!(
                root.resolve(requested, OperationKind::Read),
                Err(TargetRejection::MalformedPath),
                "malformed request must never reach the filesystem: {requested}"
            );
        }
        assert_eq!(
            root.resolve("", OperationKind::Create),
            Err(TargetRejection::MalformedPath)
        );
    }

    #[test]
    fn read_and_edit_require_an_existing_regular_file() {
        let (directory, root) = workspace();
        fs::write(directory.path().join("input.txt"), b"hello").expect("fixture write");
        let resolved = root
            .resolve("input.txt", OperationKind::Read)
            .expect("an existing file resolves");
        assert!(resolved.as_path().ends_with("input.txt"));
        assert_eq!(
            root.resolve("missing.txt", OperationKind::Read),
            Err(TargetRejection::MissingTarget)
        );
        fs::create_dir(directory.path().join("sub")).expect("fixture directory");
        assert_eq!(
            root.resolve("sub", OperationKind::Read),
            Err(TargetRejection::NotAFile)
        );
        assert_eq!(
            root.resolve("sub", OperationKind::Edit),
            Err(TargetRejection::NotAFile)
        );
    }

    #[test]
    fn create_requires_a_contained_existing_parent_and_an_absent_destination() {
        let (directory, root) = workspace();
        let resolved = root
            .resolve("report.md", OperationKind::Create)
            .expect("a fresh file in the root resolves");
        let expected = fs::canonicalize(directory.path())
            .expect("canonical root")
            .join("report.md");
        assert_eq!(resolved.as_path(), expected.to_string_lossy());
        assert_eq!(
            root.resolve("missing/report.md", OperationKind::Create),
            Err(TargetRejection::MissingParent)
        );
        fs::write(directory.path().join("exists.md"), b"old").expect("fixture write");
        assert_eq!(
            root.resolve("exists.md", OperationKind::Create),
            Err(TargetRejection::AlreadyExists)
        );
        fs::create_dir(directory.path().join("sub")).expect("fixture directory");
        assert!(
            root.resolve("sub/report.md", OperationKind::Create).is_ok(),
            "an existing subdirectory is a valid parent"
        );
        fs::create_dir_all(directory.path().join("sub/deep")).expect("fixture directory");
        assert!(
            root.resolve("sub/deep/report.md", OperationKind::Create)
                .is_ok(),
            "every existing ancestor in a nested path is walked"
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_through_an_inside_symlink_parent_resolves_inside() {
        let (directory, root) = workspace();
        let sub = directory.path().join("sub");
        fs::create_dir(&sub).expect("inside subdirectory");
        std::os::unix::fs::symlink(&sub, directory.path().join("link")).expect("inside symlink");
        let resolved = root
            .resolve("link/new.txt", OperationKind::Create)
            .expect("an inside symlink parent never leaves the workspace");
        let expected = fs::canonicalize(&sub)
            .expect("canonical subdirectory")
            .join("new.txt");
        assert_eq!(resolved.as_path(), expected.to_string_lossy());
    }

    #[cfg(unix)]
    #[test]
    fn create_refuses_a_reentry_symlink_path() {
        let (directory, root) = workspace();
        let outside = tempdir().expect("outside directory");
        let sub = directory.path().join("sub");
        fs::create_dir(&sub).expect("inside subdirectory");
        std::os::unix::fs::symlink(outside.path(), directory.path().join("out"))
            .expect("out symlink");
        std::os::unix::fs::symlink(&sub, outside.path().join("back")).expect("back symlink");
        assert_eq!(
            root.resolve("out/back/new.txt", OperationKind::Create),
            Err(TargetRejection::OutsideWorkspace),
            "an ancestor that leaves the workspace is refused even when the final parent re-enters"
        );
        assert!(
            !sub.join("new.txt").exists(),
            "a refused create never reaches the filesystem"
        );
    }

    #[cfg(windows)]
    #[test]
    fn create_refuses_a_reentry_symlink_path() {
        use std::os::windows::fs::symlink_dir;

        let (directory, root) = workspace();
        let outside = tempdir().expect("outside directory");
        let sub = directory.path().join("sub");
        fs::create_dir(&sub).expect("inside subdirectory");
        if symlink_dir(outside.path(), directory.path().join("out")).is_err()
            || symlink_dir(&sub, outside.path().join("back")).is_err()
        {
            return;
        }
        assert_eq!(
            root.resolve("out/back/new.txt", OperationKind::Create),
            Err(TargetRejection::OutsideWorkspace),
            "an ancestor that leaves the workspace is refused even when the final parent re-enters"
        );
        assert!(
            !sub.join("new.txt").exists(),
            "a refused create never reaches the filesystem"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_that_leave_the_workspace_are_refused() {
        let (directory, root) = workspace();
        let outside = tempdir().expect("outside directory");
        let outside_file = outside.path().join("secret.txt");
        fs::write(&outside_file, b"secret").expect("outside fixture");
        std::os::unix::fs::symlink(&outside_file, directory.path().join("escape.txt"))
            .expect("escape symlink");
        assert_eq!(
            root.resolve("escape.txt", OperationKind::Read),
            Err(TargetRejection::OutsideWorkspace)
        );
        std::os::unix::fs::symlink(outside.path(), directory.path().join("escape-dir"))
            .expect("escape directory symlink");
        assert_eq!(
            root.resolve("escape-dir/secret.txt", OperationKind::Read),
            Err(TargetRejection::OutsideWorkspace)
        );
        fs::write(directory.path().join("inside.txt"), b"inside").expect("inside fixture");
        std::os::unix::fs::symlink(
            directory.path().join("inside.txt"),
            directory.path().join("link.txt"),
        )
        .expect("inside symlink");
        let resolved = root
            .resolve("link.txt", OperationKind::Read)
            .expect("an inside symlink resolves to its contained target");
        assert!(resolved.as_path().ends_with("inside.txt"));
    }

    #[test]
    fn execute_reads_and_writes_with_honest_effects() {
        let (directory, root) = workspace();
        fs::write(directory.path().join("input.txt"), b"hello").expect("fixture write");

        let read_target = root
            .resolve("input.txt", OperationKind::Read)
            .expect("read target");
        let read = root.execute(&read_target, OperationKind::Read, None);
        assert_eq!(read.output, Some(ActionOutput::Bytes(b"hello".to_vec())));
        assert!(crate::attempt::certainty_grounds_pair_is_valid(
            read.certainty,
            read.grounds
        ));

        let create_target = root
            .resolve("report.md", OperationKind::Create)
            .expect("create target");
        let created = root.execute(&create_target, OperationKind::Create, Some(b"# report"));
        assert_eq!(
            created.certainty,
            crate::attempt::ActionCertainty::ConfirmedSuccess
        );
        assert_eq!(
            fs::read(directory.path().join("report.md")).expect("created file"),
            b"# report"
        );

        assert_eq!(
            root.resolve("input.txt", OperationKind::Create),
            Err(TargetRejection::AlreadyExists)
        );
        let race_target = root
            .resolve("race.md", OperationKind::Create)
            .expect("a fresh target resolves");
        fs::write(directory.path().join("race.md"), b"winner").expect("race fixture");
        let raced = root.execute(&race_target, OperationKind::Create, Some(b"loser"));
        assert_eq!(
            raced.certainty,
            crate::attempt::ActionCertainty::ConfirmedFailure
        );
        assert_eq!(
            raced.output, None,
            "a refused create never reports a success marker"
        );
        assert_eq!(
            fs::read(directory.path().join("race.md")).expect("race file kept"),
            b"winner"
        );

        let edit_target = root
            .resolve("report.md", OperationKind::Edit)
            .expect("edit target");
        let edited = root.execute(&edit_target, OperationKind::Edit, Some(b"# edited"));
        assert_eq!(
            edited.certainty,
            crate::attempt::ActionCertainty::ConfirmedSuccess
        );
        assert_eq!(edited.output, Some(ActionOutput::Updated));
        assert_eq!(
            fs::read(directory.path().join("report.md")).expect("edited file"),
            b"# edited"
        );
    }

    #[test]
    fn owned_staging_binding_rejects_a_replacement_before_body_write() {
        let (directory, root) = workspace();
        let staging_root = directory.path().join(".ene-action-staging");
        let staging = staging_root.join("attempt");
        fs::create_dir_all(&staging).expect("owned staging");
        let token = "owned-staging-token";
        fs::write(staging.join(super::STAGING_OWNERSHIP_MARKER), token).expect("owned marker");
        let identity = super::staging_identity_token(&staging).expect("owned identity");
        let moved = staging_root.join("moved");
        fs::rename(&staging, &moved).expect("rename owned staging");
        fs::create_dir(&staging).expect("replacement staging");
        fs::write(staging.join(super::STAGING_OWNERSHIP_MARKER), token)
            .expect("replacement marker");
        let target = root
            .resolve("bound.md", OperationKind::Create)
            .expect("create target");
        let options = WorkspaceEffectOptions {
            staging_directory: Some(staging.clone()),
            staging_ownership_token: Some(token.to_owned()),
            staging_identity: Some(identity),
            #[cfg(any(test, feature = "test-support"))]
            pause_after_staging: None,
        };

        let effect = root.execute_with_options(
            &target,
            OperationKind::Create,
            Some(b"target-bearing content"),
            &options,
        );

        assert_eq!(
            effect.certainty,
            crate::attempt::ActionCertainty::ConfirmedFailure
        );
        assert!(!directory.path().join("bound.md").exists());
        assert_eq!(
            fs::read_dir(&staging).expect("replacement staging").count(),
            1,
            "replacement receives only its marker, never target-bearing temp content"
        );
        assert_eq!(
            fs::read_dir(&moved).expect("owned staging").count(),
            1,
            "the owned object is left for its supervisor cleanup"
        );
    }

    #[test]
    fn bound_staging_temp_and_publish_ignore_replacement_pathname() {
        let directory = tempdir().expect("temporary directory");
        let staging = directory.path().join("staging");
        fs::create_dir(&staging).expect("owned staging");
        let token = "owned-staging-token";
        fs::write(staging.join(super::STAGING_OWNERSHIP_MARKER), token).expect("owned marker");
        let bound = super::open_bound_directory(&staging).expect("bound staging handle");
        assert_eq!(
            super::read_bound_staging_marker(&bound).as_deref(),
            Some(token)
        );

        let moved = directory.path().join("moved");
        fs::rename(&staging, &moved).expect("move owned staging");
        fs::create_dir(&staging).expect("replacement staging");
        fs::write(staging.join(super::STAGING_OWNERSHIP_MARKER), token)
            .expect("replacement marker");

        let name = std::ffi::OsString::from("bound.tmp");
        let mut temporary =
            super::create_bound_temp(&bound, &name).expect("handle-relative temporary");
        std::io::Write::write_all(&mut temporary, b"target-bearing content").expect("bound write");
        temporary.sync_all().expect("bound sync");
        let destination = directory.path().join("published.txt");
        super::publish_bound_temp(&bound, &temporary, &name, &destination, false)
            .expect("handle-relative publish");

        assert_eq!(
            fs::read(&destination).expect("published content"),
            b"target-bearing content"
        );
        assert_eq!(
            fs::read_dir(&staging).expect("replacement staging").count(),
            1,
            "replacement pathname receives only its marker"
        );
        assert_eq!(
            fs::read_dir(&moved).expect("owned staging").count(),
            1,
            "the source temp is moved out of the owned staging object"
        );
    }

    #[test]
    fn execute_maps_missing_reads_to_confirmed_failure_without_an_effect() {
        let (_directory, root) = workspace();
        let target = crate::attempt::RealTargetRef::from_canonical_path(
            root.as_path()
                .join("missing.txt")
                .to_string_lossy()
                .into_owned(),
        );
        let effect = root.execute(&target, OperationKind::Read, None);
        assert_eq!(
            effect.certainty,
            crate::attempt::ActionCertainty::ConfirmedFailure
        );
        assert_eq!(
            effect.grounds,
            crate::attempt::EffectGrounds::RefusedBeforeEffect
        );
        assert_eq!(effect.output, None);
    }

    #[test]
    fn forged_outside_targets_cannot_read_or_list_at_effect_time() {
        let (_directory, root) = workspace();
        let outside = tempdir().expect("outside directory");
        let outside_file = outside.path().join("secret.txt");
        fs::write(&outside_file, b"secret").expect("outside fixture");
        let canonical_file = fs::canonicalize(&outside_file).expect("canonical outside file");
        let forged_file = crate::attempt::RealTargetRef::from_canonical_path(
            canonical_file.to_string_lossy().into_owned(),
        );
        let read = root.execute(&forged_file, OperationKind::Read, None);
        assert_eq!(
            read.certainty,
            crate::attempt::ActionCertainty::ConfirmedFailure,
            "a forged outside target must not be readable"
        );
        assert_eq!(
            read.grounds,
            crate::attempt::EffectGrounds::RefusedBeforeEffect
        );
        assert_eq!(read.output, None);

        let canonical_dir = fs::canonicalize(outside.path()).expect("canonical outside directory");
        let forged_dir = crate::attempt::RealTargetRef::from_canonical_path(
            canonical_dir.to_string_lossy().into_owned(),
        );
        let listed = root.execute(&forged_dir, OperationKind::List, None);
        assert_eq!(
            listed.certainty,
            crate::attempt::ActionCertainty::ConfirmedFailure,
            "a forged outside directory must not be listable"
        );
        assert_eq!(listed.output, None);
    }

    #[test]
    fn debug_redacts_read_output() {
        let effect = super::ObservedEffect {
            certainty: crate::attempt::ActionCertainty::ConfirmedSuccess,
            grounds: crate::attempt::EffectGrounds::ObservedAtTarget,
            output: Some(ActionOutput::Bytes(b"secret file body".to_vec())),
        };
        let rendered = format!("{effect:?}");
        assert!(!rendered.contains("secret file body"));
        assert!(rendered.contains("bytes redacted"));
    }

    #[test]
    fn debug_redacts_listing_names() {
        let effect = super::ObservedEffect {
            certainty: crate::attempt::ActionCertainty::ConfirmedSuccess,
            grounds: crate::attempt::EffectGrounds::ObservedAtTarget,
            output: Some(ActionOutput::Listing(vec![ListEntry {
                name: String::from("private-notes.md"),
                kind: ListEntryKind::File,
            }])),
        };
        let rendered = format!("{effect:?}");
        assert!(!rendered.contains("private-notes.md"));
        assert!(rendered.contains("entries redacted"));
    }

    #[test]
    fn list_requires_a_directory_and_observes_a_sorted_listing() {
        let (directory, root) = workspace();
        fs::write(directory.path().join("b.txt"), b"b").expect("fixture write");
        fs::write(directory.path().join("a.txt"), b"a").expect("fixture write");
        fs::create_dir(directory.path().join("sub")).expect("fixture directory");
        assert_eq!(
            root.resolve("a.txt", OperationKind::List),
            Err(TargetRejection::NotADirectory)
        );
        assert_eq!(
            root.resolve("missing", OperationKind::List),
            Err(TargetRejection::MissingTarget)
        );
        let target = root
            .resolve("", OperationKind::List)
            .expect("an empty request lists the workspace root");
        assert_eq!(target.as_path(), root.as_path().to_string_lossy());
        let effect = root.execute(&target, OperationKind::List, None);
        assert_eq!(
            effect.certainty,
            crate::attempt::ActionCertainty::ConfirmedSuccess
        );
        assert_eq!(
            effect.output,
            Some(ActionOutput::Listing(vec![
                ListEntry {
                    name: String::from("a.txt"),
                    kind: ListEntryKind::File,
                },
                ListEntry {
                    name: String::from("b.txt"),
                    kind: ListEntryKind::File,
                },
                ListEntry {
                    name: String::from("sub"),
                    kind: ListEntryKind::Directory,
                },
            ]))
        );
    }

    #[cfg(unix)]
    #[test]
    fn list_excludes_symlink_entries_without_following_them() {
        let (directory, root) = workspace();
        let outside = tempdir().expect("outside directory");
        fs::write(outside.path().join("secret.txt"), b"secret").expect("outside fixture");
        std::os::unix::fs::symlink(outside.path(), directory.path().join("escape"))
            .expect("escape symlink");
        std::os::unix::fs::symlink(
            directory.path().join("inside"),
            directory.path().join("dangling"),
        )
        .expect("dangling symlink");
        let target = root
            .resolve(".", OperationKind::List)
            .expect("root listing");
        let effect = root.execute(&target, OperationKind::List, None);
        assert_eq!(
            effect.certainty,
            crate::attempt::ActionCertainty::ConfirmedSuccess
        );
        assert_eq!(
            effect.output,
            Some(ActionOutput::Listing(Vec::new())),
            "symlink entries are excluded, never followed or listed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn list_excludes_special_files() {
        let (directory, root) = workspace();
        fs::write(directory.path().join("regular.txt"), b"x").expect("fixture file");
        let _socket = std::os::unix::net::UnixListener::bind(directory.path().join("sock"))
            .expect("fixture socket");
        let target = root
            .resolve(".", OperationKind::List)
            .expect("root listing");
        let effect = root.execute(&target, OperationKind::List, None);
        assert_eq!(
            effect.output,
            Some(ActionOutput::Listing(vec![ListEntry {
                name: String::from("regular.txt"),
                kind: ListEntryKind::File,
            }]))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_mount_boundary_rejects_nested_mount_points() {
        use std::path::PathBuf;

        use super::crosses_linux_mount;

        let root = PathBuf::from("/srv/workspace");
        let mounts = vec![
            PathBuf::from("/srv"),
            PathBuf::from("/srv/workspace/nested"),
        ];
        assert!(!crosses_linux_mount(
            &root,
            &PathBuf::from("/srv/workspace"),
            &mounts
        ));
        assert!(!crosses_linux_mount(
            &root,
            &PathBuf::from("/srv/workspace/file"),
            &mounts
        ));
        assert!(crosses_linux_mount(
            &root,
            &PathBuf::from("/srv/workspace/nested"),
            &mounts
        ));
        assert!(crosses_linux_mount(
            &root,
            &PathBuf::from("/srv/workspace/nested/file"),
            &mounts
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_mountinfo_lines_decode_kernel_escapes() {
        use std::path::PathBuf;

        use super::{decode_mountinfo_escape, parse_mount_point};

        let line = "36 35 98:0 /mnt1 /srv/my\\040workspace rw,noatime master:1 - ext3 /dev/root rw,errors=continue";
        assert_eq!(
            parse_mount_point(line),
            Some(PathBuf::from("/srv/my workspace"))
        );
        assert_eq!(decode_mountinfo_escape("/a\\134b"), "/a\\b");
        assert_eq!(parse_mount_point("too short"), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_inside_targets_share_the_root_volume_serial() {
        let (directory, root) = workspace();
        fs::write(directory.path().join("input.txt"), b"hello").expect("fixture write");
        let canonical =
            fs::canonicalize(directory.path().join("input.txt")).expect("canonical fixture");
        let target_metadata = fs::metadata(&canonical).expect("target metadata");
        assert_eq!(
            WorkspaceRoot::volume_serial_of(root.as_path()),
            WorkspaceRoot::volume_serial_of(&canonical),
            "an inside target must share the root volume serial"
        );
        assert!(
            WorkspaceRoot::volume_serial_of(root.as_path()).is_some(),
            "the serial must be determinable on the test volume"
        );
        assert!(
            root.boundary_holds(&canonical, &target_metadata),
            "a same-volume inside target holds the boundary"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_reparse_points_are_excluded_and_escapes_refused() {
        use std::os::windows::fs::{symlink_dir, symlink_file};

        let (directory, root) = workspace();
        let outside = tempdir().expect("outside directory");
        fs::write(outside.path().join("secret.txt"), b"secret").expect("outside fixture");
        fs::write(directory.path().join("regular.txt"), b"x").expect("fixture file");
        if symlink_file(
            outside.path().join("secret.txt"),
            directory.path().join("escape.txt"),
        )
        .is_err()
        {
            return;
        }
        drop(symlink_dir(
            outside.path(),
            directory.path().join("escape-dir"),
        ));
        assert!(
            matches!(
                root.resolve("escape.txt", OperationKind::Read),
                Err(TargetRejection::OutsideWorkspace | TargetRejection::CrossFilesystem)
            ),
            "a reparse escape must not resolve inside the workspace"
        );
        let target = root.resolve("", OperationKind::List).expect("root listing");
        let effect = root.execute(&target, OperationKind::List, None);
        assert_eq!(
            effect.certainty,
            crate::attempt::ActionCertainty::ConfirmedSuccess
        );
        let Some(ActionOutput::Listing(entries)) = effect.output else {
            panic!("a listing observes entries");
        };
        assert!(
            entries
                .iter()
                .all(|entry| entry.name != "escape.txt" && entry.name != "escape-dir"),
            "reparse/junction entries are excluded, never followed: {entries:?}"
        );
        assert!(
            entries.iter().any(|entry| entry.name == "regular.txt"),
            "regular files remain listable: {entries:?}"
        );
    }
}
