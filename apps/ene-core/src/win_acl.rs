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
    CloseHandle, ERROR_ALREADY_EXISTS, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, LocalFree,
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

struct ProcessToken(HANDLE);

impl ProcessToken {
    fn open() -> std::io::Result<Self> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: `token` is an out-parameter the API initializes on success.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(last_error("OpenProcessToken"));
        }
        Ok(Self(token))
    }
}

impl Drop for ProcessToken {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful OpenProcessToken and is
        // closed exactly once here, on every exit path.
        unsafe { CloseHandle(self.0) };
    }
}

fn current_user_sid_string() -> std::io::Result<String> {
    let token = ProcessToken::open()?;
    let mut buffer = vec![0_u8; 128];
    let mut returned = 0_u32;
    // SAFETY: `buffer` is large enough for TOKEN_USER plus its SID header;
    // on failure we return instead of reading it.
    if unsafe {
        GetTokenInformation(
            token.0,
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

/// Creates the data directory with owner-only protection from the moment it
/// exists, plus any missing components below the nearest existing ancestor.
/// Ancestors this call did not create are never re-ACL'd; an already
/// existing data directory keeps its verify-and-repair contract.
pub(crate) fn create_owner_only_dir(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        return apply_owner_only(path);
    }
    let mut missing: Vec<&Path> = Vec::new();
    let mut current = path;
    loop {
        if current.exists() {
            if !current.is_dir() {
                return Err(occupied());
            }
            break;
        }
        missing.push(current);
        match current.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => current = parent,
            _ => break,
        }
    }
    for component in missing.iter().rev() {
        create_missing_dir(component, *component == path)?;
    }
    Ok(())
}

fn occupied() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "the data directory path is occupied by a non-directory",
    )
}

/// Creates one missing directory with the owner-only descriptor already in
/// force. A concurrent creator owns the race: an intermediate is then left
/// alone like any other ancestor, while the data directory itself is
/// re-protected.
fn create_missing_dir(path: &Path, is_data_dir: bool) -> std::io::Result<()> {
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
        return if is_data_dir {
            apply_owner_only(path)
        } else {
            Ok(())
        };
    }
    Err(last_error("CreateDirectoryW"))
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Security::ACL;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::System::Threading::GetProcessHandleCount;

    use super::*;

    fn security_sddl(path: &Path) -> String {
        let mut owner: PSID = std::ptr::null_mut();
        let mut group: PSID = std::ptr::null_mut();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut sacl: *mut ACL = std::ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: all out-pointers are live locals; the returned descriptor
        // owns the owner and DACL pointers and is freed exactly once below.
        let status = unsafe {
            GetNamedSecurityInfoW(
                wide_path(path).as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                &mut group,
                &mut dacl,
                &mut sacl,
                &mut descriptor,
            )
        };
        assert_eq!(status, 0, "the descriptor must be readable");
        let mut text: windows_sys::core::PWSTR = std::ptr::null_mut();
        let mut length = 0_u32;
        // SAFETY: `descriptor` is valid and `text` is an out-parameter the
        // conversion allocates; both are freed exactly once below.
        let converted = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut text,
                &mut length,
            )
        };
        assert_eq!(converted, 1, "the descriptor must stringify");
        let mut chars = Vec::new();
        let mut index = 0_usize;
        // SAFETY: `text` holds the NUL-terminated UTF-16 string the
        // conversion allocated, and `descriptor` outlives this copy.
        unsafe {
            while *text.add(index) != 0 {
                chars.push(*text.add(index));
                index += 1;
            }
            LocalFree(text.cast::<c_void>());
            LocalFree(descriptor.cast::<c_void>());
        }
        String::from_utf16_lossy(&chars)
    }

    fn owner_only_sddl(sid: &str) -> String {
        format!("O:{sid}D:P(A;;FA;;;{sid})")
    }

    fn handle_count() -> u32 {
        let mut count = 0_u32;
        // SAFETY: `count` is an out-parameter the API fills on success.
        assert!(
            unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) } != 0,
            "the process handle count must be readable"
        );
        count
    }

    #[test]
    fn an_existing_parent_keeps_its_acl_while_ene_directories_are_owner_only() {
        let parent = tempfile::tempdir().expect("an existing parent directory");
        let before = security_sddl(parent.path());
        let nested = parent.path().join("nested");
        let data_dir = nested.join("ene-data");

        crate::serve::lifecycle::ensure_data_dir(&data_dir)
            .expect("the nested data directory must be created");

        assert_eq!(
            security_sddl(parent.path()),
            before,
            "an existing parent must keep the descriptor it already had"
        );
        let sid = current_user_sid_string().expect("the user sid must read");
        assert_eq!(
            security_sddl(&data_dir),
            owner_only_sddl(&sid),
            "the created data directory must be owner-only"
        );
        assert_eq!(
            security_sddl(&nested),
            owner_only_sddl(&sid),
            "every created component must be owner-only"
        );

        crate::serve::lifecycle::ensure_data_dir(&data_dir)
            .expect("an existing data directory must stay usable");
        assert_eq!(
            security_sddl(&data_dir),
            owner_only_sddl(&sid),
            "an existing data directory keeps its verify-and-repair contract"
        );
    }

    #[test]
    fn repeated_sid_lookups_do_not_leak_token_handles() {
        for _ in 0..20 {
            current_user_sid_string().expect("the user sid must read");
        }
        let before = handle_count();
        for _ in 0..1000 {
            current_user_sid_string().expect("the user sid must read");
        }
        let growth = handle_count().saturating_sub(before);
        assert!(
            growth < 500,
            "1000 token reads must not grow the handle table; it grew by {growth}"
        );
    }
}
