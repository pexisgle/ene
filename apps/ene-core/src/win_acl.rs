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
    GetNamedSecurityInfoW, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, EqualSid, GetAce,
    GetTokenInformation, IsValidSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, SetFileSecurityW, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const ACCESS_ALLOWED: u8 = 0;
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

fn sid_to_string(sid: PSID) -> Option<String> {
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
    let mut token: HANDLE = 0;
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
            TOKEN_USER,
            buffer.as_mut_ptr().cast::<c_void>(),
            buffer.len() as u32,
            &mut returned,
        )
    } == 0
    {
        return Err(last_error("GetTokenInformation"));
    }
    let user = std::ptr::from_ref(buffer.as_ptr().cast::<TOKEN_USER>());
    // SAFETY: `buffer` outlives the pointer and GetTokenInformation reported
    // success, so `User.Sid` points into the filled buffer.
    unsafe { sid_to_string((*user).User.Sid) }.ok_or_else(|| last_error("ConvertSidToStringSidW"))
}

/// A descriptor that grants this user full control and nobody else, protected
/// from inherited ACEs.
fn owner_only_descriptor() -> std::io::Result<PSECURITY_DESCRIPTOR> {
    let sid = current_user_sid_string()?;
    let sddl = format!("D:P(A;;FA;;;{sid})");
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
    LocalFree(descriptor.cast::<c_void>());
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
            0,
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

/// Fail-closed protection check: the object must be owned by this user and
/// every allow ACE must grant only this user.
pub(crate) fn owner_only_ok(path: &Path) -> std::io::Result<bool> {
    let mut owner: PSID = std::ptr::null_mut();
    let mut group: PSID = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: all out-pointers are live locals; the returned descriptor is
    // freed once at the end of this function.
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
    let outcome = (|| {
        if status != 0 {
            return Err(last_error("GetNamedSecurityInfoW"));
        }
        if owner.is_null() || dacl.is_null() {
            return Ok(false);
        }
        let sid = current_user_sid_string()?;
        // SAFETY: `owner` comes from GetNamedSecurityInfoW and is valid until
        // the descriptor is freed below.
        let owner_text = unsafe { sid_to_string(owner) };
        if owner_text.as_deref() != Some(sid.as_str()) {
            return Ok(false);
        }
        // SAFETY: `dacl` comes from GetNamedSecurityInfoW and is valid until
        // the descriptor is freed below.
        let ace_count = unsafe { (*dacl).AceCount };
        for index in 0..ace_count {
            let mut ace: *mut c_void = std::ptr::null_mut();
            // SAFETY: `dacl` is valid and `ace` is an out-pointer.
            if unsafe { GetAce(dacl, u32::from(index), &mut ace) } == 0 {
                return Ok(false);
            }
            let header = ace.cast::<ACE_HEADER>();
            // SAFETY: `ace` addresses at least the ACE header written by GetAce.
            if unsafe { (*header).AceType } != ACCESS_ALLOWED {
                // Fail closed: this process only ever creates plain allow
                // ACEs, so anything else is not one of our files.
                return Ok(false);
            }
            let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
            // SAFETY: for an allow ACE the SID begins at SidStart; the ACE
            // body written by GetAce covers it.
            let ace_sid = unsafe { std::ptr::from_ref(&(*allowed).SidStart) }.cast::<c_void>();
            // SAFETY: pointer provenance as above; sid_to_string validates
            // the SID before reading it.
            let ace_text = unsafe { sid_to_string(ace_sid) };
            if ace_text.as_deref() != Some(sid.as_str()) {
                return Ok(false);
            }
        }
        Ok(true)
    })();
    if !descriptor.is_null() {
        // SAFETY: descriptor was allocated by GetNamedSecurityInfoW and is
        // freed exactly once here.
        unsafe { LocalFree(descriptor.cast::<c_void>()) };
    }
    outcome
}
