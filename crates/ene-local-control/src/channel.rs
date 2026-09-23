pub const CONFIRMATION_MODE_ENV: &str = "ENE_CONFIRMATION_CHANNEL";
pub const CONFIRMATION_MODE_STDIO: &str = "stdio";

pub const MAX_CONTROL_FRAME_BYTES: u32 = 16 * 1024;

pub fn encode_body<T: serde::Serialize>(value: &T) -> std::io::Result<Vec<u8>> {
    let body = serde_json::to_vec(value)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if body.len() > MAX_CONTROL_FRAME_BYTES as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "control frame exceeds the bound",
        ));
    }
    Ok(body)
}

pub fn decode_body<T: serde::de::DeserializeOwned>(body: &[u8]) -> std::io::Result<T> {
    serde_json::from_slice(body)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChannelEvent {
    Inbound(ToConfirmation),
    Outbound(FromConfirmation),
    Closed,
}

use crate::{FromConfirmation, ToConfirmation};

pub fn encode_frame<T: serde::Serialize>(value: &T) -> std::io::Result<Vec<u8>> {
    let body = zeroize::Zeroizing::new(encode_body(value)?);
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

pub fn read_frame<R, T>(reader: &mut R) -> std::io::Result<Option<T>>
where
    R: std::io::Read,
    T: serde::de::DeserializeOwned,
{
    let mut prefix = [0_u8; 4];
    match reader.read_exact(&mut prefix) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = u32::from_be_bytes(prefix);
    if length == 0 || length > MAX_CONTROL_FRAME_BYTES {
        return Ok(None);
    }
    let mut body = zeroize::Zeroizing::new(vec![0_u8; length as usize]);
    reader.read_exact(&mut body)?;
    decode_body(&body).map(Some)
}

pub fn write_frame<W, T>(writer: &mut W, value: &T) -> std::io::Result<()>
where
    W: std::io::Write,
    T: serde::Serialize,
{
    let bytes = zeroize::Zeroizing::new(encode_frame(value)?);
    writer.write_all(&bytes)?;
    writer.flush()
}

#[cfg(unix)]
mod platform {
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
    use std::os::unix::net::UnixStream;

    use crate::{FromConfirmation, ToConfirmation};

    pub struct HostChannel {
        stream: UnixStream,
    }

    impl HostChannel {
        pub fn try_clone(&self) -> std::io::Result<Self> {
            Ok(Self {
                stream: self.stream.try_clone()?,
            })
        }

        pub fn pair() -> std::io::Result<(Self, ChildHandles)> {
            let (host, gui) = UnixStream::pair()?;
            Ok((
                Self { stream: host },
                ChildHandles {
                    child: Some(OwnedFd::from(gui)),
                },
            ))
        }

        pub fn recv(&mut self) -> std::io::Result<Option<ToConfirmation>> {
            super::read_frame(&mut self.stream)
        }

        pub fn send(&mut self, frame: &FromConfirmation) -> std::io::Result<()> {
            super::write_frame(&mut self.stream, frame)
        }
    }

    pub struct ChildHandles {
        child: Option<OwnedFd>,
    }

    impl ChildHandles {
        pub fn apply(&mut self, command: &mut std::process::Command) {
            if let Some(fd) = self.child.take() {
                command.stdin(std::process::Stdio::from(fd));
            }
        }
    }

    pub struct GuiChannel {
        stream: UnixStream,
    }

    impl GuiChannel {
        pub fn try_clone(&self) -> std::io::Result<Self> {
            Ok(Self {
                stream: self.stream.try_clone()?,
            })
        }

        #[doc(hidden)]
        pub fn pair_for_test() -> std::io::Result<(Self, HostChannel)> {
            let (host, gui) = UnixStream::pair()?;
            Ok((Self { stream: gui }, HostChannel { stream: host }))
        }

        pub fn adopt_stdio() -> std::io::Result<Self> {
            // SAFETY: fd 0 is this process's stdin, which the Host set to the
            // child end of the pair before exec. Taking ownership here keeps a
            // single owner.
            let fd = unsafe { OwnedFd::from_raw_fd(0) };
            // The Host's dup2 during spawn cleared FD_CLOEXEC; re-arm it so no
            // exec'd descendant inherits the confirmation endpoint.
            // SAFETY: `fd` is a valid open descriptor and `fcntl` with F_SETFD
            // does not transfer ownership.
            if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self {
                stream: UnixStream::from(fd),
            })
        }

        pub fn recv(&mut self) -> std::io::Result<Option<FromConfirmation>> {
            super::read_frame(&mut self.stream)
        }

        pub fn send(&mut self, frame: &ToConfirmation) -> std::io::Result<()> {
            super::write_frame(&mut self.stream, frame)
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::fs::File;
    use std::os::windows::io::{FromRawHandle as _, OwnedHandle};

    use windows_sys::Win32::Foundation::{
        HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
    };
    use windows_sys::Win32::System::Pipes::CreatePipe;

    use crate::{FromConfirmation, ToConfirmation};

    fn anonymous_pipe() -> std::io::Result<(OwnedHandle, OwnedHandle)> {
        let mut read: HANDLE = INVALID_HANDLE_VALUE;
        let mut write: HANDLE = INVALID_HANDLE_VALUE;
        // SAFETY: both out-pointers are valid for the call; a null descriptor
        // asks for the default attributes and size 0 asks for the default
        // buffer size. On success each out-parameter owns one fresh handle.
        let created =
            unsafe { CreatePipe(&raw mut read, &raw mut write, core::ptr::null_mut(), 0) };
        if created == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `CreatePipe` returned two distinct fresh handles owned by
        // this call; each `OwnedHandle` takes exactly one.
        Ok(unsafe {
            (
                OwnedHandle::from_raw_handle(read),
                OwnedHandle::from_raw_handle(write),
            )
        })
    }

    pub struct HostChannel {
        from_gui: File,
        to_gui: File,
    }

    impl HostChannel {
        pub fn try_clone(&self) -> std::io::Result<Self> {
            Ok(Self {
                from_gui: self.from_gui.try_clone()?,
                to_gui: self.to_gui.try_clone()?,
            })
        }

        pub fn pair() -> std::io::Result<(Self, ChildHandles)> {
            let (gui_read, host_write) = anonymous_pipe()?;
            let (host_read, gui_write) = anonymous_pipe()?;
            Ok((
                Self {
                    from_gui: File::from(host_read),
                    to_gui: File::from(host_write),
                },
                ChildHandles {
                    stdin: Some(File::from(gui_read)),
                    stdout: Some(File::from(gui_write)),
                },
            ))
        }

        pub fn recv(&mut self) -> std::io::Result<Option<ToConfirmation>> {
            super::read_frame(&mut self.from_gui)
        }

        pub fn send(&mut self, frame: &FromConfirmation) -> std::io::Result<()> {
            super::write_frame(&mut self.to_gui, frame)
        }
    }

    pub struct ChildHandles {
        stdin: Option<File>,
        stdout: Option<File>,
    }

    impl ChildHandles {
        pub fn apply(&mut self, command: &mut std::process::Command) {
            if let Some(stdin) = self.stdin.take() {
                command.stdin(std::process::Stdio::from(stdin));
            }
            if let Some(stdout) = self.stdout.take() {
                command.stdout(std::process::Stdio::from(stdout));
            }
        }
    }

    pub struct GuiChannel {
        from_host: File,
        to_host: File,
    }

    impl GuiChannel {
        pub fn try_clone(&self) -> std::io::Result<Self> {
            Ok(Self {
                from_host: self.from_host.try_clone()?,
                to_host: self.to_host.try_clone()?,
            })
        }

        #[doc(hidden)]
        pub fn pair_for_test() -> std::io::Result<(Self, HostChannel)> {
            let (host_read, gui_write) = anonymous_pipe()?;
            let (gui_read, host_write) = anonymous_pipe()?;
            Ok((
                Self {
                    from_host: File::from(gui_read),
                    to_host: File::from(gui_write),
                },
                HostChannel {
                    from_gui: File::from(host_read),
                    to_gui: File::from(host_write),
                },
            ))
        }

        pub fn adopt_stdio() -> std::io::Result<Self> {
            use std::os::windows::io::{AsHandle as _, AsRawHandle as _};

            let from_host = std::io::stdin().as_handle().try_clone_to_owned()?;
            let to_host = std::io::stdout().as_handle().try_clone_to_owned()?;
            for handle in [
                std::io::stdin().as_raw_handle(),
                std::io::stdout().as_raw_handle(),
            ] {
                // SAFETY: `handle` is a live process standard handle; the call
                // only clears the inherit flag and does not take ownership.
                if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(Self {
                from_host: File::from(from_host),
                to_host: File::from(to_host),
            })
        }

        pub fn recv(&mut self) -> std::io::Result<Option<FromConfirmation>> {
            super::read_frame(&mut self.from_host)
        }

        pub fn send(&mut self, frame: &ToConfirmation) -> std::io::Result<()> {
            super::write_frame(&mut self.to_host, frame)
        }
    }
}

pub use platform::{ChildHandles, GuiChannel, HostChannel};

#[cfg(test)]
mod tests {
    use super::{MAX_CONTROL_FRAME_BYTES, encode_frame, read_frame, write_frame};
    use crate::ToConfirmation;

    #[test]
    fn an_oversize_frame_is_refused() {
        let huge = "x".repeat(MAX_CONTROL_FRAME_BYTES as usize + 1);
        assert!(encode_frame(&huge).is_err());
    }

    #[test]
    fn frames_round_trip() {
        let mut buffer = Vec::new();
        let frame = ToConfirmation::SessionComplete {
            session_id: uuid::Uuid::nil(),
            nonce: crate::RedactedSecret::new("n"),
        };
        write_frame(&mut buffer, &frame).expect("encode");
        let mut cursor = std::io::Cursor::new(buffer);
        let decoded: Option<ToConfirmation> = read_frame(&mut cursor).expect("decode");
        assert_eq!(decoded, Some(frame));
    }

    #[test]
    fn a_truncated_frame_ends_the_channel() {
        let mut cursor = std::io::Cursor::new(vec![0_u8, 0, 0, 8, b'x']);
        let decoded: Result<Option<ToConfirmation>, _> = read_frame(&mut cursor);
        assert!(
            decoded.is_err(),
            "a truncated body must surface as a failure, got {decoded:?}"
        );
    }
}
