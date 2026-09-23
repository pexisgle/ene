//! Owner-only file and directory creation for the WSS runtime information.
//!
//! IPC §10.4 requires the runtime file (which carries the local token) and
//! its staging file to be readable and writable by the same OS user from the
//! moment they exist; Windows gets that guarantee from an explicit security
//! descriptor at creation, never from the default ACL.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::FromRawHandle as _;
use std::path::Path;

use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetTokenInformation, IsValidSid, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SetFileSecurityW, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const SDDL_REVISION_1: u32 = 1;

fn wide(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn last_error(context: &str) -> std::io::Error {
    // SAFETY: reading the thread's last Win32 error is always valid.
    let code = unsafe { GetLastError() };
    std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("{context}: Win32 error {code}"),
    )
}

unsafe fn sid_to_string(sid: PSID) -> Option<String> {
    // SAFETY: callers pass pointers obtained from GetNamedSecurityInfoW or
    // GetTokenInformation; IsValidSid validates them before the read.
    if unsafe { IsValidSid(sid) } == 0 {
        return None;
    }
    let mut text: windows_sys::core::PWSTR = std::ptr::null_mut();
    // SAFETY: `sid` is valid per the check above and `text` is an
    // out-parameter the conversion allocates on success.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return None;
    }
    let mut len = 0_usize;
    // SAFETY: `text` is a NUL-terminated UTF-16 string allocated by the
    // conversion above.
    while unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `text` holds at least `len + 1` code units as checked.
    let owned = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
    // SAFETY: `text` was allocated by ConvertSidToStringSidW and is freed
    // exactly once here after the copy is taken.
    unsafe { LocalFree(text.cast::<c_void>()) };
    Some(owned)
}

fn current_user_sid_string() -> std::io::Result<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `token` is an out-parameter the API initializes on success.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(last_error("OpenProcessToken"));
    }
    let mut buffer = vec![0_u8; 128];
    let mut returned = 0_u32;
    // SAFETY: `buffer` is large enough for TOKEN_USER plus its SID header;
    // on failure we return instead of reading it.
    if unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast::<c_void>(),
            buffer.len() as u32,
            &mut returned,
        )
    } == 0
    {
        return Err(last_error("GetTokenInformation"));
    }
    // SAFETY: `buffer` outlives the pointer and GetTokenInformation reported
    // success, so `User.Sid` points into the filled buffer.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    // SAFETY: `user` references the filled buffer, so `User.Sid` is a live
    // SID for `sid_to_string` to read.
    unsafe { sid_to_string((*user).User.Sid) }.ok_or_else(|| last_error("ConvertSidToStringSidW"))
}

/// A descriptor that names this user as owner and grants them full control
/// and nobody else, protected from inherited ACEs.
fn owner_only_descriptor() -> std::io::Result<PSECURITY_DESCRIPTOR> {
    let sid = current_user_sid_string()?;
    let sddl = format!("O:{sid}D:P(A;;FA;;;{sid})");
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let mut length = 0_u32;
    // SAFETY: inputs are our own wide strings and out-pointers; the caller
    // frees the returned descriptor with LocalFree.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide(&sddl).as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            &mut length,
        )
    };
    if ok == 0 {
        return Err(last_error(
            "ConvertStringSecurityDescriptorToSecurityDescriptorW",
        ));
    }
    Ok(descriptor)
}

fn attributes(descriptor: PSECURITY_DESCRIPTOR) -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    }
}

/// # Safety
/// `descriptor` must come from `owner_only_descriptor` and stay alive until
/// the returned handle is closed.
unsafe fn free_descriptor(descriptor: PSECURITY_DESCRIPTOR) {
    // SAFETY: forwarded contract from the caller; freed exactly once.
    unsafe { LocalFree(descriptor.cast::<c_void>()) };
}

pub(crate) fn create_owner_only(path: &Path) -> std::io::Result<std::fs::File> {
    let descriptor = owner_only_descriptor()?;
    let attributes = attributes(descriptor);
    // SAFETY: wide paths are NUL-terminated and the descriptor outlives the
    // call; the handle is validated before conversion.
    let handle = unsafe {
        CreateFileW(
            wide_path(path).as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_NONE,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    // SAFETY: descriptor came from owner_only_descriptor and is freed once.
    unsafe { free_descriptor(descriptor) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateFileW"));
    }
    // SAFETY: `handle` is a fresh, valid file handle we now own exclusively.
    Ok(unsafe { std::fs::File::from_raw_handle(handle as _) })
}

fn apply_owner_only(path: &Path) -> std::io::Result<()> {
    let descriptor = owner_only_descriptor()?;
    // SAFETY: both the path and descriptor are valid for this call.
    let result = unsafe {
        SetFileSecurityW(
            wide_path(path).as_ptr(),
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            descriptor,
        )
    };
    // SAFETY: descriptor came from owner_only_descriptor and is freed once.
    unsafe { free_descriptor(descriptor) };
    if result == 0 {
        return Err(last_error("SetFileSecurityW"));
    }
    Ok(())
}

pub(crate) fn create_owner_only_dir(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        return apply_owner_only(path);
    }
    if path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "the data directory path is occupied by a non-directory",
        ));
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !path.exists()
    {
        create_owner_only_dir(parent)?;
    }
    let descriptor = owner_only_descriptor()?;
    let attributes = attributes(descriptor);
    // SAFETY: wide path and descriptor are valid for this call.
    let created = unsafe { CreateDirectoryW(wide_path(path).as_ptr(), &attributes) };
    // SAFETY: descriptor came from owner_only_descriptor and is freed once.
    unsafe { free_descriptor(descriptor) };
    if created != 0 {
        return Ok(());
    }
    // SAFETY: GetLastError is always readable.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return apply_owner_only(path);
    }
    Err(last_error("CreateDirectoryW"))
}
