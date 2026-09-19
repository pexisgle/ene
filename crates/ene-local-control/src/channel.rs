//! The private confirmation channel between the serving Host and the GUI it
//! spawned (Stage 7 A1, [First-party desktop] §5.1.2).
//!
//! The Host creates the pair while spawning the official GUI and hands the
//! child end to that child alone: on Linux an unnamed socket-pair end carried
//! as the child's stdin, on Windows a pair of anonymous pipes carried as the
//! child's stdin and stdout. No other handle is made inheritable, so nothing
//! else reaches the child, and nothing names the channel, so no other process
//! can dial it.
//!
//! The channel is blocking by design. It carries a handful of small frames
//! per Owner action, never stream data, and each side runs it on its own
//! thread so a silent peer can never stall an event loop.
//!
//! [First-party desktop]: ../../../../docs/design/concrete/first-party-desktop.md

// The frame types live in this crate's root; the platform modules import the
// ones they need.

/// Environment marker the Host sets on the spawned GUI. Its absence means the
/// process was started by the user as the short-lived launcher.
pub const CONFIRMATION_MODE_ENV: &str = "ENE_CONFIRMATION_CHANNEL";

/// Marker value: the confirmation channel is on this process's stdio.
pub const CONFIRMATION_MODE_STDIO: &str = "stdio";

/// Upper bound on one channel frame. The secret-bearing credential frame is
/// the largest legal message; anything bigger is not this protocol.
pub const MAX_CONFIRMATION_FRAME_BYTES: u32 = 16 * 1024;

/// One confirmation-channel event, either direction.
///
/// The Host merges its own outbound frames and the GUI's inbound frames into
/// one queue so a single thread owns the channel and no lock is held across a
/// blocking read.
#[derive(Debug, Clone, PartialEq)]
pub enum ChannelEvent {
    /// A frame the GUI sent to the Host.
    Inbound(ToConfirmation),
    /// A frame the Host is sending to the GUI.
    Outbound(FromConfirmation),
    /// The channel ended (clean EOF, malformed frame, or transport failure).
    Closed,
}

use crate::{FromConfirmation, ToConfirmation};

/// Encodes one frame as a `u32` big-endian length followed by JSON.
///
/// # Errors
///
/// Fails when the value cannot serialize or exceeds
/// [`MAX_CONFIRMATION_FRAME_BYTES`].
pub fn encode_frame<T: serde::Serialize>(value: &T) -> std::io::Result<Vec<u8>> {
    let body = serde_json::to_vec(value)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
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
/// `Ok(None)` means the peer closed, or the frame was malformed or oversize.
/// Both are terminal for the channel: a malformed frame never becomes a
/// guessed message.
///
/// # Errors
///
/// Propagates transport failures other than a clean EOF.
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
    let mut body = vec![0_u8; length as usize];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

/// Writes one framed message.
///
/// # Errors
///
/// Propagates encoding and transport failures.
pub fn write_frame<W, T>(writer: &mut W, value: &T) -> std::io::Result<()>
where
    W: std::io::Write,
    T: serde::Serialize,
{
    let bytes = encode_frame(value)?;
    writer.write_all(&bytes)?;
    writer.flush()
}

#[cfg(unix)]
mod platform {
    use std::os::fd::{FromRawFd as _, OwnedFd};
    use std::os::unix::net::UnixStream;

    use crate::{FromConfirmation, ToConfirmation};

    /// The Host's end of one spawned GUI's private channel.
    pub struct HostChannel {
        stream: UnixStream,
    }

    impl HostChannel {
        /// Duplicates this end so one thread can read while another writes.
        ///
        /// # Errors
        ///
        /// Returns the OS failure when the descriptor cannot be duplicated.
        pub fn try_clone(&self) -> std::io::Result<Self> {
            Ok(Self {
                stream: self.stream.try_clone()?,
            })
        }

        /// Creates one private pair plus the child ends to pass as stdio.
        ///
        /// # Errors
        ///
        /// Returns the OS failure when the socket pair cannot be created.
        pub fn pair() -> std::io::Result<(Self, ChildHandles)> {
            let (host, child) = UnixStream::pair()?;
            Ok((Self { stream: host }, ChildHandles { stdin: Some(child) }))
        }

        /// Reads one frame from the GUI.
        ///
        /// # Errors
        ///
        /// Propagates transport failures other than a clean EOF.
        pub fn recv(&mut self) -> std::io::Result<Option<ToConfirmation>> {
            super::read_frame(&mut self.stream)
        }

        /// Sends one frame to the GUI.
        ///
        /// # Errors
        ///
        /// Propagates encoding and transport failures.
        pub fn send(&mut self, frame: &FromConfirmation) -> std::io::Result<()> {
            super::write_frame(&mut self.stream, frame)
        }
    }

    /// The child ends of one private pair, applied to a spawn.
    pub struct ChildHandles {
        stdin: Option<UnixStream>,
    }

    impl ChildHandles {
        /// Applies the pair to `command`'s stdio.
        ///
        /// Only these ends are handed to the child: the socket pair was made
        /// close-on-exec, so no other descriptor of this process travels.
        pub fn apply(&mut self, command: &mut std::process::Command) {
            if let Some(stream) = self.stdin.take() {
                command.stdin(std::process::Stdio::from(OwnedFd::from(stream)));
            }
        }
    }

    /// The GUI's end of the inherited private channel.
    pub struct GuiChannel {
        stream: UnixStream,
    }

    impl GuiChannel {
        /// Duplicates this end so one thread can read while the owner writes.
        ///
        /// # Errors
        ///
        /// Returns the OS failure when the descriptor cannot be duplicated.
        pub fn try_clone(&self) -> std::io::Result<Self> {
            Ok(Self {
                stream: self.stream.try_clone()?,
            })
        }

        /// Creates a connected pair for tests that drive a GUI in-process.
        ///
        /// The production GUI always adopts the Host's stdio; this constructor
        /// exists so a test can act as the spawned child without a process
        /// boundary, and is never used by product code.
        ///
        /// # Errors
        ///
        /// Returns the OS failure when the socket pair cannot be created.
        #[doc(hidden)]
        pub fn pair_for_test() -> std::io::Result<(Self, HostChannel)> {
            let (host, gui) = UnixStream::pair()?;
            Ok((Self { stream: gui }, HostChannel { stream: host }))
        }

        /// Adopts the channel the Host passed as this process's stdin.
        ///
        /// # Errors
        ///
        /// Returns the OS failure when stdin cannot be taken.
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

        /// Reads one frame from the Host.
        ///
        /// # Errors
        ///
        /// Propagates transport failures other than a clean EOF.
        pub fn recv(&mut self) -> std::io::Result<Option<FromConfirmation>> {
            super::read_frame(&mut self.stream)
        }

        /// Sends one frame to the Host.
        ///
        /// # Errors
        ///
        /// Propagates encoding and transport failures.
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

    /// One anonymous pipe pair as `(read end, write end)`.
    ///
    /// The default security attributes are used deliberately: a pipe end is
    /// inheritable only because the child's stdio carries it. The Host marks
    /// no other handle inheritable, so nothing else reaches the child.
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

    /// The Host's end of one spawned GUI's private channel.
    pub struct HostChannel {
        from_gui: File,
        to_gui: File,
    }

    impl HostChannel {
        /// Duplicates this end so one thread can read while another writes.
        ///
        /// # Errors
        ///
        /// Returns the OS failure when a handle cannot be duplicated.
        pub fn try_clone(&self) -> std::io::Result<Self> {
            Ok(Self {
                from_gui: self.from_gui.try_clone()?,
                to_gui: self.to_gui.try_clone()?,
            })
        }

        /// Creates one private pair plus the child ends to pass as stdio.
        ///
        /// # Errors
        ///
        /// Returns the OS failure when a pipe cannot be created.
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

        /// Reads one frame from the GUI.
        ///
        /// # Errors
        ///
        /// Propagates transport failures other than a clean EOF.
        pub fn recv(&mut self) -> std::io::Result<Option<ToConfirmation>> {
            super::read_frame(&mut self.from_gui)
        }

        /// Sends one frame to the GUI.
        ///
        /// # Errors
        ///
        /// Propagates encoding and transport failures.
        pub fn send(&mut self, frame: &FromConfirmation) -> std::io::Result<()> {
            super::write_frame(&mut self.to_gui, frame)
        }
    }

    /// The child ends of one private pair, applied to a spawn.
    pub struct ChildHandles {
        stdin: Option<File>,
        stdout: Option<File>,
    }

    impl ChildHandles {
        /// Applies the pair to `command`'s stdio.
        ///
        /// The Host marks no other handle inheritable, so these two are the
        /// only handles that reach the child.
        pub fn apply(&mut self, command: &mut std::process::Command) {
            if let Some(stdin) = self.stdin.take() {
                command.stdin(std::process::Stdio::from(stdin));
            }
            if let Some(stdout) = self.stdout.take() {
                command.stdout(std::process::Stdio::from(stdout));
            }
        }
    }

    /// The GUI's end of the inherited private channel.
    ///
    /// The host-spawned GUI takes its channel from stdio directly, so no raw
    /// handle is retained here: the standard streams own them for the process
    /// lifetime and the channel is never re-handed onward.
    pub struct GuiChannel {
        from_host: File,
        to_host: File,
    }

    impl GuiChannel {
        /// Duplicates this end so one thread can read while the owner writes.
        ///
        /// # Errors
        ///
        /// Returns the OS failure when a handle cannot be duplicated.
        pub fn try_clone(&self) -> std::io::Result<Self> {
            Ok(Self {
                from_host: self.from_host.try_clone()?,
                to_host: self.to_host.try_clone()?,
            })
        }

        /// Creates a connected pair for tests that drive a GUI in-process.
        ///
        /// The production GUI always adopts the Host's stdio; this constructor
        /// exists so a test can act as the spawned child without a process
        /// boundary, and is never used by product code.
        ///
        /// # Errors
        ///
        /// Returns the OS failure when the pipes cannot be created.
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

        /// Adopts the channel the Host passed as this process's stdio.
        ///
        /// # Errors
        ///
        /// Returns the OS failure when the standard handles are unusable.
        pub fn adopt_stdio() -> std::io::Result<Self> {
            use std::os::windows::io::AsHandle as _;

            let from_host = std::io::stdin().as_handle().try_clone_to_owned()?;
            let to_host = std::io::stdout().as_handle().try_clone_to_owned()?;
            Ok(Self {
                from_host: File::from(from_host),
                to_host: File::from(to_host),
            })
        }

        /// Reads one frame from the Host.
        ///
        /// # Errors
        ///
        /// Propagates transport failures other than a clean EOF.
        pub fn recv(&mut self) -> std::io::Result<Option<FromConfirmation>> {
            super::read_frame(&mut self.from_host)
        }

        /// Sends one frame to the Host.
        ///
        /// # Errors
        ///
        /// Propagates encoding and transport failures.
        pub fn send(&mut self, frame: &ToConfirmation) -> std::io::Result<()> {
            super::write_frame(&mut self.to_host, frame)
        }
    }
}

pub use platform::{ChildHandles, GuiChannel, HostChannel};

#[cfg(test)]
mod tests {
    use super::{MAX_CONFIRMATION_FRAME_BYTES, encode_frame, read_frame, write_frame};
    use crate::ToConfirmation;

    /// A frame that is too large is refused, never truncated.
    #[test]
    fn an_oversize_frame_is_refused() {
        let huge = "x".repeat(MAX_CONFIRMATION_FRAME_BYTES as usize + 1);
        assert!(encode_frame(&huge).is_err());
    }

    /// Frames round-trip through the encoder, so both ends agree on shape.
    #[test]
    fn frames_round_trip() {
        let mut buffer = Vec::new();
        let frame = ToConfirmation::SessionComplete {
            session_id: uuid::Uuid::nil(),
            nonce: String::from("n"),
        };
        write_frame(&mut buffer, &frame).expect("encode");
        let mut cursor = std::io::Cursor::new(buffer);
        let decoded: Option<ToConfirmation> = read_frame(&mut cursor).expect("decode");
        assert_eq!(decoded, Some(frame));
    }

    /// A frame whose declared length is not there ends the channel rather than
    /// guessing at partial content. A truncated body is a transport failure,
    /// not a message: the reader reports it and the caller closes the channel.
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
