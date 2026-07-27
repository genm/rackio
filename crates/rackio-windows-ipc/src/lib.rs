#![cfg(windows)]
// Named-pipe DACL construction and caller-token verification have no safe Win32
// wrapper. This is the only crate in the workspace permitted to use `unsafe`.
#![allow(unsafe_code)]

use std::{
    ffi::{OsStr, c_void},
    io,
    os::windows::{ffi::OsStrExt as _, io::AsRawHandle as _},
    ptr,
};

use tokio::{
    net::windows::named_pipe::{NamedPipeClient, NamedPipeServer, ServerOptions},
    time::{Duration, sleep},
};
use windows_sys::{
    Win32::{
        Foundation::{
            ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, ERROR_PIPE_BUSY, GetLastError, HANDLE,
            INVALID_HANDLE_VALUE, LocalFree,
        },
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SDDL_REVISION_1,
            },
            CheckTokenMembership, CreateWellKnownSid, LookupAccountNameW, PSECURITY_DESCRIPTOR,
            PSID, RevertToSelf, SECURITY_MAX_SID_SIZE, WinBuiltinAdministratorsSid,
        },
        Storage::FileSystem::{
            CreateFileW, FILE_FLAG_OVERLAPPED, FILE_READ_ATTRIBUTES, FILE_READ_DATA,
            FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, OPEN_EXISTING, SYNCHRONIZE,
        },
        System::Pipes::ImpersonateNamedPipeClient,
    },
    core::PWSTR,
};

const PIPE_SDDL_PREFIX: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x100183;;;";
pub const DEFAULT_PIPE_NAME: &str = r"\\.\pipe\rackio-agent";
pub const VIEWER_GROUP_NAME: &str = "Rackio Viewers";

pub fn configured_pipe_name() -> io::Result<String> {
    let name = std::env::var("RACKIO_PIPE").unwrap_or_else(|_| String::from(DEFAULT_PIPE_NAME));
    let suffix = name.strip_prefix(r"\\.\pipe\").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            r"RACKIO_PIPE must start with \\.\pipe\",
        )
    })?;
    if suffix.is_empty() || suffix.contains(['\\', '/']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "RACKIO_PIPE must contain one non-empty local pipe name",
        ));
    }
    Ok(name)
}

pub async fn connect_client(pipe_name: &str) -> io::Result<NamedPipeClient> {
    let pipe_name = wide(pipe_name);
    for attempt in 0..20 {
        // Use only data/attribute/synchronize rights. Unlike GENERIC_WRITE,
        // this intentionally excludes FILE_CREATE_PIPE_INSTANCE so viewer
        // group members cannot impersonate the daemon with another instance.
        let desired_access = FILE_READ_DATA
            | FILE_WRITE_DATA
            | FILE_READ_ATTRIBUTES
            | FILE_WRITE_ATTRIBUTES
            | SYNCHRONIZE;
        // SAFETY: The name is nul-terminated, no optional pointers are
        // dereferenced, and a successful owned handle is transferred directly
        // into Tokio below.
        let handle = unsafe {
            CreateFileW(
                pipe_name.as_ptr(),
                desired_access,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        };
        let opened = if handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: CreateFileW returned a unique overlapped named-pipe
            // handle, and ownership is transferred exactly once.
            unsafe { NamedPipeClient::from_raw_handle(handle.cast()) }
        };
        match opened {
            Ok(client) => return Ok(client),
            Err(error)
                if attempt < 19
                    && matches!(
                        error.raw_os_error().map(i32::cast_unsigned),
                        Some(ERROR_PIPE_BUSY | ERROR_FILE_NOT_FOUND)
                    ) =>
            {
                sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "named pipe did not become available",
    ))
}

/// Owns the Windows security descriptor and group SID used by the local pipe.
///
/// Unsafe Win32 pointer handling is isolated in this crate; callers only get a
/// safe API that creates a local-only pipe and verifies the connected token.
pub struct PipeSecurity {
    descriptor: PSECURITY_DESCRIPTOR,
    viewer_group_sid: Vec<u8>,
    administrators_sid: Vec<u8>,
}

// SAFETY: this type has unique ownership of a LocalAlloc security descriptor
// and its SID buffers. Windows permits LocalFree on a different thread, and
// the type is deliberately not Sync, so no descriptor is shared concurrently.
unsafe impl Send for PipeSecurity {}

impl PipeSecurity {
    pub fn for_local_group(group_name: &str) -> io::Result<Self> {
        let viewer_group_sid = lookup_account_sid(group_name)?;
        let sid = sid_to_string(viewer_group_sid.as_ptr().cast_mut().cast())?;
        let sddl = format!("{PIPE_SDDL_PREFIX}{sid})");
        let descriptor = security_descriptor(&sddl)?;
        let administrators_sid = well_known_sid(WinBuiltinAdministratorsSid)?;
        Ok(Self {
            descriptor,
            viewer_group_sid,
            administrators_sid,
        })
    }

    pub fn create_server(
        &self,
        options: &ServerOptions,
        pipe_name: impl AsRef<OsStr>,
    ) -> io::Result<NamedPipeServer> {
        let mut attributes = windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>())
                .unwrap_or(u32::MAX),
            lpSecurityDescriptor: self.descriptor,
            bInheritHandle: 0,
        };
        // SAFETY: `attributes` and the owned descriptor remain valid for the
        // complete CreateNamedPipeW call. Tokio does not retain the pointer.
        unsafe {
            options.create_with_security_attributes_raw(
                pipe_name,
                ptr::from_mut(&mut attributes).cast::<c_void>(),
            )
        }
    }

    /// Verifies the token associated with the most recent read from `pipe`.
    pub fn verify_client_after_read(&self, pipe: &NamedPipeServer) -> io::Result<()> {
        let handle = pipe.as_raw_handle() as HANDLE;
        // Inspect the security context attached to this pipe connection
        // directly. Reopening a process by PID introduces a race with client
        // exit and can fail even when the pipe token itself is queryable.
        // Windows requires the server to read data before impersonation so it
        // can bind the check to the client that supplied that data.
        // SAFETY: `handle` is a live connected named-pipe server handle.
        if unsafe { ImpersonateNamedPipeClient(handle) } == 0 {
            return Err(io::Error::last_os_error());
        }

        let authorized =
            token_is_member(ptr::null_mut(), &self.viewer_group_sid).and_then(|viewer| {
                if viewer {
                    Ok(true)
                } else {
                    token_is_member(ptr::null_mut(), &self.administrators_sid)
                }
            });
        // SAFETY: This thread successfully impersonated the connected client
        // above and must return to the daemon identity before doing more work.
        if unsafe { RevertToSelf() } == 0 {
            // Returning an error would leave this pooled worker thread running
            // as the viewer for every later task. There is no way to drop the
            // impersonation token, so the only fail-closed outcome is to stop.
            let error = io::Error::last_os_error();
            eprintln!("rackio: could not drop client impersonation ({error}); aborting");
            std::process::abort();
        }
        if !authorized? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "named-pipe caller is neither a Rackio viewer nor an administrator",
            ));
        }
        Ok(())
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        if !self.descriptor.is_null() {
            // SAFETY: ConvertStringSecurityDescriptor allocated this pointer
            // with LocalAlloc and this is its single owner.
            unsafe {
                LocalFree(self.descriptor.cast());
            }
        }
    }
}

fn token_is_member(token: HANDLE, sid: &[u8]) -> io::Result<bool> {
    let mut is_member = 0;
    // SAFETY: The token handle and SID buffer remain valid for this call.
    if unsafe { CheckTokenMembership(token, sid.as_ptr().cast_mut().cast(), &raw mut is_member) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(is_member != 0)
}

fn well_known_sid(kind: i32) -> io::Result<Vec<u8>> {
    let mut size = SECURITY_MAX_SID_SIZE;
    let mut sid = vec![0_u8; size as usize];
    // SAFETY: `sid` has the documented maximum SID size and the size pointer
    // references initialized writable storage.
    if unsafe {
        CreateWellKnownSid(
            kind,
            ptr::null_mut(),
            sid.as_mut_ptr().cast(),
            &raw mut size,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    sid.truncate(size as usize);
    Ok(sid)
}

fn lookup_account_sid(name: &str) -> io::Result<Vec<u8>> {
    let name = wide(name);
    let mut sid_size = 0_u32;
    let mut domain_size = 0_u32;
    let mut use_type = 0_i32;
    // SAFETY: Null output buffers intentionally request the required sizes.
    unsafe {
        LookupAccountNameW(
            ptr::null(),
            name.as_ptr(),
            ptr::null_mut(),
            &raw mut sid_size,
            ptr::null_mut(),
            &raw mut domain_size,
            &raw mut use_type,
        );
    }
    // SAFETY: GetLastError has no preconditions and reads thread-local state.
    if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
        return Err(io::Error::last_os_error());
    }

    let mut sid = vec![0_u8; sid_size as usize];
    let mut domain = vec![0_u16; domain_size as usize];
    // SAFETY: Both buffers have the sizes requested by the first call and all
    // output pointers are valid for their declared element types.
    if unsafe {
        LookupAccountNameW(
            ptr::null(),
            name.as_ptr(),
            sid.as_mut_ptr().cast(),
            &raw mut sid_size,
            domain.as_mut_ptr(),
            &raw mut domain_size,
            &raw mut use_type,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(sid)
}

fn sid_to_string(sid: PSID) -> io::Result<String> {
    let mut value: PWSTR = ptr::null_mut();
    // SAFETY: `sid` points into the caller-owned valid SID buffer and `value`
    // is a writable output pointer.
    if unsafe { ConvertSidToStringSidW(sid, &raw mut value) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = wide_ptr_to_string(value);
    // SAFETY: ConvertSidToStringSidW allocated `value` with LocalAlloc.
    unsafe {
        LocalFree(value.cast());
    }
    result
}

fn security_descriptor(sddl: &str) -> io::Result<PSECURITY_DESCRIPTOR> {
    let sddl = wide(sddl);
    let mut descriptor = ptr::null_mut();
    // SAFETY: The UTF-16 string is nul-terminated and `descriptor` is a valid
    // writable output pointer.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &raw mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(descriptor)
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(Some(0)).collect()
}

fn wide_ptr_to_string(value: PWSTR) -> io::Result<String> {
    if value.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows returned a null string",
        ));
    }
    let mut len = 0;
    // SAFETY: Windows returned a valid nul-terminated PWSTR.
    unsafe {
        while *value.add(len) != 0 {
            len += 1;
        }
        String::from_utf16(&*ptr::slice_from_raw_parts(value, len))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}
