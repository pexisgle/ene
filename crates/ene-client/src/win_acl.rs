//! Owner-only creation and verification for the client-side protected files
//! (the trusted Host pin). Mirrors the Host's runtime-file rules from
//! IPC §10.4 / §10.5: Windows never trusts the default ACL.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::FromRawHandle as _;
use std::path::Path;

use windows_sys::Win32::Foundation::{
    GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetNamedSecurityInfoW, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce, GetTokenInformation,
    IsValidSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE,
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

/// Creates the file with an owner-only descriptor in force from the first
/// byte written; the file must not already exist.
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
    unsafe { LocalFree(descriptor.cast::<c_void>()) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateFileW"));
    }
    // SAFETY: `handle` is a fresh, valid file handle we now own exclusively.
    Ok(unsafe { std::fs::File::from_raw_handle(handle as _) })
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
                // ACEs, so anything else is not one of ours.
                return Ok(false);
            }
            let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
            // SAFETY: for an allow ACE the SID begins at SidStart; the ACE
            // body written by GetAce covers it.
            let ace_sid = unsafe { std::ptr::addr_of_mut!((*allowed).SidStart) }.cast::<c_void>();
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
