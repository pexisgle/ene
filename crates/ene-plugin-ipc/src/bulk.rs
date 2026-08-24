//! Out-of-band bulk transfer. Frames carry references; bytes and FDs travel
//! beside the `MessagePack` envelope.

use crate::protocol::DEFAULT_BULK_THRESHOLD_BYTES;

/// True when `len` should leave the `MessagePack` frame.
#[must_use]
pub fn should_spill(len: usize, threshold: u32) -> bool {
    let cap = if threshold == 0 {
        DEFAULT_BULK_THRESHOLD_BYTES
    } else {
        threshold
    };
    u32::try_from(len).map_or(true, |n| n > cap)
}

#[cfg(unix)]
mod unix {
    use std::io;
    use std::mem::size_of;
    use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};

    const MAX_FDS: usize = 8;
    const DUMMY: [u8; 1] = [0x42];

    fn cmsg_len(fd_count: usize) -> usize {
        // SAFETY: `CMSG_LEN` is a constant-time size calculation for `fd_count`
        // file descriptors; it does not dereference pointers.
        usize::try_from(unsafe { libc::CMSG_LEN((fd_count * size_of::<RawFd>()) as libc::c_uint) })
            .unwrap_or(0)
    }

    fn cmsg_space(fd_count: usize) -> usize {
        // SAFETY: `CMSG_SPACE` is a constant-time size calculation for `fd_count`
        // file descriptors; it does not dereference pointers.
        usize::try_from(unsafe {
            libc::CMSG_SPACE((fd_count * size_of::<RawFd>()) as libc::c_uint)
        })
        .unwrap_or(0)
    }

    /// Send `fds` as `SCM_RIGHTS` ancillary data on `socket`.
    ///
    /// # Errors
    ///
    /// Returns I/O errors from `sendmsg`, or `InvalidInput` when `fds` is empty
    /// or longer than eight.
    pub fn send_fds(socket: impl AsFd, fds: &[RawFd]) -> io::Result<()> {
        if fds.is_empty() || fds.len() > MAX_FDS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "bulk FD count must be 1..=8",
            ));
        }
        let sock = socket.as_fd().as_raw_fd();
        let mut dummy = DUMMY;
        let mut iov = libc::iovec {
            iov_base: dummy.as_mut_ptr().cast(),
            iov_len: dummy.len(),
        };
        let space = cmsg_space(fds.len());
        let mut cmsg_buf = vec![0_u8; space];
        let msg = libc::msghdr {
            msg_name: std::ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: &raw mut iov,
            msg_iovlen: 1,
            msg_control: cmsg_buf.as_mut_ptr().cast(),
            msg_controllen: space,
            msg_flags: 0,
        };
        // SAFETY: `msg` points at live `iov` and `cmsg_buf`. `CMSG_FIRSTHDR` /
        // `CMSG_DATA` stay inside `cmsg_buf`. `fds` is copied into that buffer
        // before `sendmsg`. The socket fd is valid for the duration of the call.
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&raw const msg);
            if cmsg.is_null() {
                return Err(io::Error::other("CMSG_FIRSTHDR returned null"));
            }
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = cmsg_len(fds.len());
            std::ptr::copy_nonoverlapping(
                fds.as_ptr().cast::<u8>(),
                libc::CMSG_DATA(cmsg),
                fds.len().saturating_mul(size_of::<RawFd>()),
            );
            let n = libc::sendmsg(sock, &raw const msg, 0);
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    /// Receive up to eight `SCM_RIGHTS` file descriptors from `socket`.
    ///
    /// # Errors
    ///
    /// Returns I/O errors from `recvmsg`.
    pub fn recv_fds(socket: impl AsFd) -> io::Result<Vec<OwnedFd>> {
        let sock = socket.as_fd().as_raw_fd();
        let mut dummy = [0_u8; 1];
        let mut iov = libc::iovec {
            iov_base: dummy.as_mut_ptr().cast(),
            iov_len: dummy.len(),
        };
        let space = cmsg_space(MAX_FDS);
        let mut cmsg_buf = vec![0_u8; space];
        let mut msg = libc::msghdr {
            msg_name: std::ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: &raw mut iov,
            msg_iovlen: 1,
            msg_control: cmsg_buf.as_mut_ptr().cast(),
            msg_controllen: space,
            msg_flags: 0,
        };
        // SAFETY: buffers outlive `recvmsg`. Returned fds are owned by this
        // process after a successful `SCM_RIGHTS` receive; wrapping them in
        // `OwnedFd` takes that ownership exactly once.
        unsafe {
            let n = libc::recvmsg(sock, &raw mut msg, 0);
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            let mut out = Vec::new();
            let mut cmsg = libc::CMSG_FIRSTHDR(&raw const msg);
            while !cmsg.is_null() {
                if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                    let bytes = (*cmsg).cmsg_len.saturating_sub(cmsg_len(0));
                    let count = bytes / size_of::<RawFd>();
                    // `CMSG_DATA` is only byte-aligned; `read_unaligned`
                    // handles the kernel-packed fd array safely.
                    #[expect(
                        clippy::cast_ptr_alignment,
                        reason = "fd payloads are byte-aligned and read via read_unaligned"
                    )]
                    let data = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                    for i in 0..count {
                        let raw = std::ptr::read_unaligned(data.add(i));
                        out.push(OwnedFd::from_raw_fd(raw));
                    }
                }
                cmsg = libc::CMSG_NXTHDR(&raw const msg, cmsg);
            }
            Ok(out)
        }
    }
}

#[cfg(unix)]
pub use unix::{recv_fds, send_fds};
