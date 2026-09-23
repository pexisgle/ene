use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::RawHandle;
use std::path::Path;

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SID_AND_ATTRIBUTES,
    TOKEN_GROUPS, TOKEN_QUERY, TOKEN_USER, TokenGroups, TokenUser,
};
use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::core::{PCWSTR, PWSTR};

use crate::serve::CoreError;

const SE_GROUP_LOGON_ID: u32 = 0xC000_0000;

#[must_use]
pub fn pipe_name(data_dir: &Path) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;
    let mut tag = FNV_OFFSET;
    for byte in data_dir.as_os_str().as_encoded_bytes() {
        tag ^= u64::from(*byte);
        tag = tag.wrapping_mul(FNV_PRIME);
    }
    format!(r"\\.\pipe\ene-{tag:016x}")
}

fn wide_null(text: &str) -> Vec<u16> {
    OsStr::new(text)
        .encode_wide()
        .chain([0])
        .collect::<Vec<u16>>()
}

struct LogonSidAttrs {
    sd: PSECURITY_DESCRIPTOR,
    attrs: SECURITY_ATTRIBUTES,
}

impl LogonSidAttrs {
    fn build() -> Result<Self, CoreError> {
        let sid_text =
            logon_sid_text().ok_or_else(|| CoreError::Bind(String::from("read the logon SID")))?;
        let sddl = format!("D:P(A;;GA;;;{sid_text})");
        let wide = wide_null(&sddl);
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `wide` is null-terminated and lives for the call; `sd`
        // receives a `LocalAlloc`-ed descriptor owned by us on success.
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr() as PCWSTR,
                SDDL_REVISION_1,
                &raw mut sd,
                std::ptr::null_mut(),
            )
        };
        if converted == 0 || sd.is_null() {
            return Err(CoreError::Bind(String::from(
                "convert the logon-SID security descriptor",
            )));
        }
        let attrs = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        Ok(Self { sd, attrs })
    }

    fn as_mut_ptr(&mut self) -> *mut core::ffi::c_void {
        (&raw mut self.attrs).cast::<core::ffi::c_void>()
    }
}

impl Drop for LogonSidAttrs {
    fn drop(&mut self) {
        // SAFETY: `sd` came from `ConvertStringSecurityDescriptor...` (LocalAlloc).
        unsafe {
            LocalFree(self.sd);
        }
    }
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: every instance wraps a real handle from `OpenProcess` /
        // `OpenProcessToken`, owned until here.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn open_process_token(process: HANDLE) -> Option<OwnedHandle> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `token` is a valid out-pointer for the duration of the call.
    let opened = unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) };
    if opened == 0 || token.is_null() {
        return None;
    }
    Some(OwnedHandle(token))
}

fn token_info_bytes(token: HANDLE, class: i32) -> Option<Vec<u8>> {
    let mut needed = 0u32;
    // SAFETY: sizing call; the null buffer with zero length always fails with
    // the required length written back (or a real failure with none).
    unsafe {
        GetTokenInformation(token, class, std::ptr::null_mut(), 0, &raw mut needed);
    }
    if needed == 0 {
        return None;
    }
    let mut bytes = vec![0u8; needed as usize];
    // SAFETY: `bytes` is exactly `needed` long and lives for the call.
    let fetched = unsafe {
        GetTokenInformation(
            token,
            class,
            bytes.as_mut_ptr().cast::<core::ffi::c_void>(),
            needed,
            &raw mut needed,
        )
    };
    if fetched == 0 { None } else { Some(bytes) }
}

fn sid_to_string(sid: PSID) -> Option<String> {
    let mut wide: PWSTR = std::ptr::null_mut();
    // SAFETY: `wide` is a valid out-pointer; the returned string is
    // `LocalAlloc`-ed and owned by us on success.
    let converted = unsafe { ConvertSidToStringSidW(sid, &raw mut wide) };
    if converted == 0 || wide.is_null() {
        return None;
    }
    // SAFETY: `wide` is a null-terminated UTF-16 string until freed below.
    let text = unsafe {
        let mut length = 0usize;
        while *wide.add(length) != 0 {
            length += 1;
        }
        String::from_utf16_lossy(core::slice::from_raw_parts(wide, length))
    };
    // SAFETY: `wide` came from `ConvertSidToStringSidW` (LocalAlloc).
    unsafe {
        LocalFree(wide.cast::<core::ffi::c_void>());
    }
    Some(text)
}

fn logon_sid_text() -> Option<String> {
    // SAFETY: `GetCurrentProcess` needs no cleanup (pseudo-handle, never closed).
    let process = unsafe { GetCurrentProcess() };
    let token = open_process_token(process)?;
    let bytes = token_info_bytes(token.0, TokenGroups)?;
    // SAFETY: `bytes` holds exactly the `TOKEN_GROUPS` the OS wrote: one
    // header plus `GroupCount` trailing entries, all within this allocation.
    // The buffer is only 1-aligned (`Vec<u8>`), so every field goes through
    // an unaligned read.
    unsafe {
        let groups = bytes.as_ptr().cast::<TOKEN_GROUPS>();
        let count = core::ptr::addr_of!((*groups).GroupCount).read_unaligned() as usize;
        let first = core::ptr::addr_of!((*groups).Groups).cast::<SID_AND_ATTRIBUTES>();
        for index in 0..count {
            let entry = first.add(index).read_unaligned();
            if entry.Attributes & SE_GROUP_LOGON_ID != 0 {
                return sid_to_string(entry.Sid);
            }
        }
    }
    None
}

fn process_user_sid_text(process: HANDLE) -> Option<String> {
    let token = open_process_token(process)?;
    let bytes = token_info_bytes(token.0, TokenUser)?;
    // SAFETY: `bytes` holds exactly the `TOKEN_USER` the OS wrote.
    let sid = unsafe {
        bytes
            .as_ptr()
            .cast::<TOKEN_USER>()
            .read_unaligned()
            .User
            .Sid
    };
    sid_to_string(sid)
}

pub fn peer_same_user(pipe: RawHandle) -> bool {
    let mut pid = 0u32;
    // SAFETY: `pipe` is a live server instance owned by the listener loop;
    // `pid` is a valid out-pointer for the call.
    let known = unsafe { GetNamedPipeClientProcessId(pipe as HANDLE, &raw mut pid) };
    if known == 0 {
        return false;
    }
    peer_same_user_pid(pid)
}

#[must_use]
pub fn peer_process_id(pipe: RawHandle) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: `pipe` is a live server instance owned by the listener loop;
    // `pid` is a valid out-pointer for the call.
    let known = unsafe { GetNamedPipeClientProcessId(pipe as HANDLE, &raw mut pid) };
    if known == 0 || pid == 0 {
        None
    } else {
        Some(pid)
    }
}

fn peer_same_user_pid(pid: u32) -> bool {
    // SAFETY: `PROCESS_QUERY_LIMITED_INFORMATION` is the least privilege that
    // still admits `OpenProcessToken(TOKEN_QUERY)` on the result.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return false;
    }
    let process = OwnedHandle(process);
    // SAFETY: `GetCurrentProcess` needs no cleanup (pseudo-handle).
    let current = unsafe { GetCurrentProcess() };
    match (
        process_user_sid_text(process.0),
        process_user_sid_text(current),
    ) {
        (Some(peer), Some(own)) => peer == own,
        _ => false,
    }
}

pub fn create_first_server(pipe: &str) -> Result<NamedPipeServer, CoreError> {
    create_server(pipe, true)
}

pub fn create_next_server(pipe: &str) -> Result<NamedPipeServer, CoreError> {
    create_server(pipe, false)
}

fn create_server(pipe: &str, first: bool) -> Result<NamedPipeServer, CoreError> {
    let mut attrs = LogonSidAttrs::build()?;
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true);
    // SAFETY: `attrs` owns a valid `SECURITY_ATTRIBUTES` for the duration of
    // this synchronous call (`CreateNamedPipeW` copies the descriptor).
    unsafe { options.create_with_security_attributes_raw(pipe, attrs.as_mut_ptr()) }
        .map_err(|error| CoreError::Bind(format!("create named pipe: {error}")))
}
