pub const CONFIRMATION_MODE_ENV: &str = "ENE_CONFIRMATION_CHANNEL";
pub const CONFIRMATION_MODE_STDIO: &str = "stdio";
pub const MAX_CONFIRMATION_FRAME_BYTES: u32 = 16 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum ChannelEvent {
    Inbound(ToConfirmation),
    Outbound(FromConfirmation),
    Closed,
}

use crate::{FromConfirmation, ToConfirmation};

pub fn encode_frame<T: serde::Serialize>(value: &T) -> std::io::Result<Vec<u8>> {
    let body = zeroize::Zeroizing::new(
        serde_json::to_vec(value)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
    );
    if body.len() > MAX_CONFIRMATION_FRAME_BYTES as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "confirmation frame exceeds the bound",
        ));
    }
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Reads one framed message.
///
/// `Ok(None)` is terminal for the channel and means the peer closed or the
/// length prefix was zero or over [`MAX_CONFIRMATION_FRAME_BYTES`].
///
/// # Errors
///
/// Propagates transport failures, including a truncated body, and returns
/// `InvalidData` for a body that fails to decode; both are terminal for the
/// channel, and a malformed frame never becomes a guessed message.
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
    if length == 0 || length > MAX_CONFIRMATION_FRAME_BYTES {
        return Ok(None);
    }
    let mut body = zeroize::Zeroizing::new(vec![0_u8; length as usize]);
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
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
    use std::os::fd::{FromRawFd, OwnedFd};
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
            // single owner and never re-hands the channel to Body, tools, or
            // plugins.
            let fd = unsafe { OwnedFd::from_raw_fd(0) };
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

    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
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
            use std::os::windows::io::AsHandle as _;

            let from_host = std::io::stdin().as_handle().try_clone_to_owned()?;
            let to_host = std::io::stdout().as_handle().try_clone_to_owned()?;
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
