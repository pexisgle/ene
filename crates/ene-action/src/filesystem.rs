//! The Workspace-contained filesystem execution boundary (E-1/E-2, K-H).
//!
//! A [`WorkspaceRoot`] is a canonicalized folder. [`WorkspaceRoot::resolve`]
//! turns one requested workspace-relative path into a [`RealTargetRef`]: a
//! canonical absolute path proven to be inside the folder and on the same
//! filesystem entity at resolution time. Absolute paths, parent-directory
//! components, symlink resolutions that leave the folder, and targets that
//! cross a mount/reparse/volume boundary are refused; string equality of the
//! request is never treated as identity.
//!
//! [`WorkspaceRoot::execute`] re-verifies containment immediately before the
//! effect and publishes writes atomically in the destination directory
//! (`persist_noclobber` for create, `persist` for edit), then reads the
//! destination back. `List` observes a non-recursive directory enumeration.
//! Confirmed success means the executor observed the intended result at the
//! target; an agent self-report is never a ground.
//!
//! The guarantee is against the agent-requested path, not against an
//! unbounded concurrent local writer: a same-user process swapping directory
//! components mid-operation is outside the container the OS gives this
//! process, and an `openat2`-style syscall boundary is platform work this
//! slice does not claim. The mount/reparse check is a static boundary check
//! of the resolved path, not a live mount-table watcher.

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use tempfile::NamedTempFile;
use thiserror::Error;

use crate::attempt::{ActionCertainty, EffectGrounds, OperationKind, RealTargetRef};

/// Maximum bytes for one read result or one create/edit payload.
///
/// The bound is checked before any durable claim, so an over-limit request is
/// a never-started refusal and leaves no attempt row.
pub const MAX_ACTION_FILE_BYTES: usize = 1_048_576;

/// Failure to open the workspace folder itself.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WorkspaceRootError {
    /// The folder does not exist or cannot be canonicalized.
    #[error("workspace folder is unavailable")]
    Unavailable,
    /// The path exists but is not a directory.
    #[error("workspace folder is not a directory")]
    NotADirectory,
}

/// One directory entry observed by a `List`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntry {
    pub name: String,
    pub kind: ListEntryKind,
}

/// The kind of one listed directory entry.
///
/// Only regular files and directories are listable; symlinks, reparse points,
/// junctions, mounts, and special files are excluded from the listing (never
/// followed), so they have no kind here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListEntryKind {
    File,
    Directory,
}

/// The observed output of one action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutput {
    /// The bytes read by a `Read`.
    Bytes(Vec<u8>),
    /// The entries observed by a `List`, sorted by name.
    Listing(Vec<ListEntry>),
}

/// Why one requested path was refused before any attempt was claimed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TargetRejection {
    /// Empty, non-UTF-8, or containing `..`/absolute/root components.
    #[error("requested path is malformed")]
    MalformedPath,
    /// The canonical resolution left the workspace folder (including symlinks).
    #[error("requested path escapes the workspace")]
    OutsideWorkspace,
    /// The resolved target crosses the workspace root's filesystem entity (a
    /// nested mount, reparse point, or volume boundary), or the boundary could
    /// not be determined on this platform.
    #[error("requested target crosses the workspace filesystem boundary")]
    CrossFilesystem,
    /// List named a target that does not exist.
    #[error("requested target does not exist")]
    MissingTarget,
    /// Create named a target whose parent directory does not exist.
    #[error("requested target parent does not exist")]
    MissingParent,
    /// Read/edit named a non-regular file.
    #[error("requested target is not a regular file")]
    NotAFile,
    /// List named a non-directory.
    #[error("requested target is not a directory")]
    NotADirectory,
    /// Create named a target that already exists.
    #[error("requested target already exists")]
    AlreadyExists,
    /// The read target exceeds [`MAX_ACTION_FILE_BYTES`].
    #[error("requested target is too large")]
    TooLarge,
    /// The filesystem refused to answer (permission or I/O failure).
    #[error("requested target is unavailable")]
    TargetUnavailable,
}

/// A canonical workspace folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRoot {
    root: PathBuf,
}

impl WorkspaceRoot {
    /// Opens and canonicalizes one workspace folder.
    ///
    /// The folder is an external locator ene does not own: opening only
    /// proves it is currently a canonical directory, not availability.
    pub fn open(folder: &str) -> Result<Self, WorkspaceRootError> {
        let root = fs::canonicalize(folder).map_err(|_| WorkspaceRootError::Unavailable)?;
        if !root.is_dir() {
            return Err(WorkspaceRootError::NotADirectory);
        }
        Ok(Self { root })
    }

    /// The canonical folder path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.root
    }

    /// Resolves one requested workspace-relative path to a real target.
    ///
    /// List accepts an empty request or `.` to enumerate the workspace root.
    /// Read/edit require an existing regular file inside the folder; list
    /// requires an existing directory; create requires an existing directory
    /// parent inside the folder and a destination that does not exist yet. The
    /// returned target is the canonical absolute path, which is what
    /// execution uses.
    pub fn resolve(
        &self,
        requested: &str,
        operation: OperationKind,
    ) -> Result<RealTargetRef, TargetRejection> {
        let names =
            if operation == OperationKind::List && (requested.is_empty() || requested == ".") {
                Vec::new()
            } else {
                requested_components(requested).ok_or(TargetRejection::MalformedPath)?
            };
        match operation {
            OperationKind::List | OperationKind::Read | OperationKind::Edit => {
                let mut joined = self.root.clone();
                for name in &names {
                    joined.push(name);
                }
                let canonical = fs::canonicalize(&joined).map_err(|error| map_io_error(&error))?;
                if !canonical.starts_with(&self.root) {
                    return Err(TargetRejection::OutsideWorkspace);
                }
                let metadata = fs::metadata(&canonical).map_err(|error| map_io_error(&error))?;
                if !self.boundary_holds(&canonical, &metadata) {
                    return Err(TargetRejection::CrossFilesystem);
                }
                match operation {
                    OperationKind::List => {
                        if !metadata.is_dir() {
                            return Err(TargetRejection::NotADirectory);
                        }
                    }
                    _ => {
                        if !metadata.is_file() {
                            return Err(TargetRejection::NotAFile);
                        }
                        if operation == OperationKind::Read
                            && metadata.len() > MAX_ACTION_FILE_BYTES as u64
                        {
                            return Err(TargetRejection::TooLarge);
                        }
                    }
                }
                Ok(canonical_target(canonical))
            }
            OperationKind::Create => {
                let Some((file_name, parent_names)) = names.split_last() else {
                    return Err(TargetRejection::MalformedPath);
                };
                let mut parent = self.root.clone();
                for name in parent_names {
                    parent.push(name);
                }
                let canonical_parent = fs::canonicalize(&parent).map_err(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        TargetRejection::MissingParent
                    } else {
                        TargetRejection::TargetUnavailable
                    }
                })?;
                if !canonical_parent.starts_with(&self.root) {
                    return Err(TargetRejection::OutsideWorkspace);
                }
                if !canonical_parent.is_dir() {
                    return Err(TargetRejection::MissingParent);
                }
                let parent_metadata =
                    fs::metadata(&canonical_parent).map_err(|error| map_io_error(&error))?;
                if !self.boundary_holds(&canonical_parent, &parent_metadata) {
                    return Err(TargetRejection::CrossFilesystem);
                }
                let destination = canonical_parent.join(file_name);
                match fs::symlink_metadata(&destination) {
                    Ok(_) => return Err(TargetRejection::AlreadyExists),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(map_io_error(&error)),
                }
                Ok(canonical_target(destination))
            }
        }
    }

    /// Executes one already-resolved operation and observes the effect.
    ///
    /// Reads return the observed bytes, listings return the observed entries.
    /// Writes publish atomically in the destination directory and read the
    /// destination back; a verified read-back is the confirmed-success ground,
    /// a refusal before publishing is a confirmed failure, and anything the
    /// executor cannot verify stays [`ActionCertainty::Unknown`].
    ///
    /// `content` is required for create/edit and ignored for list/read; the
    /// orchestration checks this before any durable claim.
    ///
    /// This is `pub(crate)`: the only public effect path is
    /// [`orchestrate_workspace_action`](crate::orchestrate_workspace_action),
    /// which binds the target to the K-B.1 live decision and the AU5 start
    /// claim before executing. An externally forged [`RealTargetRef`] (built
    /// from an arbitrary string via the store read-back constructor) cannot
    /// reach an effect from another crate. Effect-time containment is
    /// re-verified for every operation and fails closed.
    #[must_use]
    pub(crate) fn execute(
        &self,
        target: &RealTargetRef,
        operation: OperationKind,
        content: Option<&[u8]>,
    ) -> ObservedEffect {
        match operation {
            OperationKind::List => self.list_directory(target),
            OperationKind::Read => {
                let destination = Path::new(target.as_path());
                let Some(metadata) = self.verified_existing_metadata(destination, false) else {
                    return refused();
                };
                if metadata.len() > MAX_ACTION_FILE_BYTES as u64 {
                    return refused();
                }
                match fs::read(destination) {
                    Ok(bytes) => ObservedEffect {
                        certainty: ActionCertainty::ConfirmedSuccess,
                        grounds: EffectGrounds::ObservedAtTarget,
                        output: Some(ActionOutput::Bytes(bytes)),
                    },
                    Err(_) => refused(),
                }
            }
            OperationKind::Create => {
                self.write_atomically(target, content.unwrap_or_default(), false)
            }
            OperationKind::Edit => self.write_atomically(target, content.unwrap_or_default(), true),
        }
    }

    /// Observes one directory as a sorted, non-recursive entry listing.
    ///
    /// Each direct child is classified from no-follow metadata: symlinks,
    /// Windows reparse points, junctions, mounts/cross-device entries, and
    /// special files are excluded from the result rather than followed or
    /// mapped to `file`/`dir`; an excluded child never fails the whole
    /// listing. A partial read of the directory is a confirmed refusal (a
    /// listing changes nothing).
    fn list_directory(&self, target: &RealTargetRef) -> ObservedEffect {
        let destination = Path::new(target.as_path());
        if self.verified_existing_metadata(destination, true).is_none() {
            return refused();
        }
        let Ok(entries) = fs::read_dir(destination) else {
            return refused();
        };
        let mut listing = Vec::new();
        for entry in entries {
            let Ok(entry) = entry else {
                return refused();
            };
            let Some(kind) = self.listable_child(&entry.path()) else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            listing.push(ListEntry { name, kind });
        }
        listing.sort_by(|left, right| left.name.cmp(&right.name));
        ObservedEffect {
            certainty: ActionCertainty::ConfirmedSuccess,
            grounds: EffectGrounds::ObservedAtTarget,
            output: Some(ActionOutput::Listing(listing)),
        }
    }

    /// Classifies one direct child of a listing without following it.
    ///
    /// `None` means the entry is excluded: a symlink or reparse point (no
    /// traversal), a mount or cross-device directory (outside the root's
    /// filesystem entity), a special file, or an entry whose metadata cannot
    /// be read (fail closed).
    fn listable_child(&self, path: &Path) -> Option<ListEntryKind> {
        let metadata = fs::symlink_metadata(path).ok()?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return None;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            // Any reparse point (symlink, junction, mount point) is excluded.
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return None;
            }
        }
        let kind = if file_type.is_file() {
            ListEntryKind::File
        } else if file_type.is_dir() {
            ListEntryKind::Directory
        } else {
            return None;
        };
        if !self.boundary_holds(path, &metadata) {
            return None;
        }
        Some(kind)
    }

    fn write_atomically(
        &self,
        target: &RealTargetRef,
        bytes: &[u8],
        replace: bool,
    ) -> ObservedEffect {
        let destination = Path::new(target.as_path());
        // Re-verify the resolved target at the effect moment: a component or
        // symlink swapped after resolution must not redirect the write.
        if !self.reverifies_at_effect(destination, replace) {
            return refused();
        }
        let Some(parent) = destination.parent() else {
            return refused();
        };
        let mut temporary = match NamedTempFile::new_in(parent) {
            Ok(temporary) => temporary,
            Err(_) => return refused(),
        };
        {
            let file = temporary.as_file_mut();
            if file.write_all(bytes).is_err() || file.sync_all().is_err() {
                // The temporary file is removed on drop; the destination was
                // never touched, so this is a confirmed pre-effect refusal.
                return refused();
            }
        }
        let persisted = if replace {
            temporary.persist(destination)
        } else {
            temporary.persist_noclobber(destination)
        };
        let persisted = match persisted {
            Ok(file) => file,
            // Publish failed atomically: the destination is untouched (for
            // create, another writer may have won; for edit, the original
            // remains), so the intended effect did not happen.
            Err(_) => return refused(),
        };
        if persisted.sync_all().is_err() {
            // The rename landed but the content durability is unconfirmed.
            return ObservedEffect {
                certainty: ActionCertainty::Unknown,
                grounds: EffectGrounds::OutcomeUnverified,
                output: None,
            };
        }
        match fs::read(destination) {
            Ok(read_back) if read_back == bytes => ObservedEffect {
                certainty: ActionCertainty::ConfirmedSuccess,
                grounds: EffectGrounds::ObservedAtTarget,
                output: None,
            },
            // Something is at the destination but not what we intended; an
            // effect occurred, but it cannot be confirmed as the intended one.
            _ => ObservedEffect {
                certainty: ActionCertainty::Unknown,
                grounds: EffectGrounds::OutcomeUnverified,
                output: None,
            },
        }
    }

    /// Best-effort re-verification immediately before the effect.
    ///
    /// Edit requires the target to still canonicalize to itself inside the
    /// root and remain on the root's filesystem entity; create requires the
    /// canonical parent to still be inside the root and on the same entity,
    /// and the destination to still be absent.
    ///
    /// Read and list use [`Self::verified_existing_metadata`]: the stored
    /// target must still canonicalize to itself, stay inside the root, and
    /// remain on the root's filesystem entity. A forged [`RealTargetRef`]
    /// pointing outside the workspace (even on the same device) is refused.
    fn verified_existing_metadata(
        &self,
        destination: &Path,
        want_directory: bool,
    ) -> Option<fs::Metadata> {
        let canonical = fs::canonicalize(destination).ok()?;
        if canonical != destination || !canonical.starts_with(&self.root) {
            return None;
        }
        let metadata = fs::metadata(&canonical).ok()?;
        if metadata.is_dir() != want_directory {
            return None;
        }
        if !self.boundary_holds(&canonical, &metadata) {
            return None;
        }
        Some(metadata)
    }

    fn reverifies_at_effect(&self, destination: &Path, replace: bool) -> bool {
        if replace {
            match (fs::canonicalize(destination), fs::metadata(destination)) {
                (Ok(canonical), Ok(metadata)) => {
                    canonical == destination
                        && canonical.starts_with(&self.root)
                        && self.boundary_holds(&canonical, &metadata)
                }
                _ => false,
            }
        } else {
            let Some(parent) = destination.parent() else {
                return false;
            };
            match (fs::canonicalize(parent), fs::metadata(parent)) {
                (Ok(canonical_parent), Ok(metadata)) => {
                    canonical_parent == parent
                        && canonical_parent.starts_with(&self.root)
                        && self.boundary_holds(&canonical_parent, &metadata)
                        && fs::symlink_metadata(destination).is_err()
                }
                _ => false,
            }
        }
    }

    /// Whether `target` is on the same filesystem entity as the workspace root
    /// and no nested mount/reparse boundary lies between them.
    ///
    /// Linux: root and target must share a device, and no mount point from
    /// `/proc/self/mountinfo` may sit strictly below the root on the target's
    /// path (an unreadable mount table fails closed). Other Unix: device
    /// equality. Windows: volume serial number equality. Undeterminable
    /// boundaries are refused, never assumed inside.
    fn boundary_holds(&self, target: &Path, metadata: &fs::Metadata) -> bool {
        self.boundary_holds_impl(target, metadata)
    }

    #[cfg(unix)]
    fn boundary_holds_impl(&self, target: &Path, metadata: &fs::Metadata) -> bool {
        use std::os::unix::fs::MetadataExt;

        let Ok(root_metadata) = fs::metadata(&self.root) else {
            return false;
        };
        if root_metadata.dev() != metadata.dev() {
            return false;
        }
        #[cfg(target_os = "linux")]
        {
            match linux_mount_points() {
                Ok(mounts) => !crosses_linux_mount(&self.root, target, &mounts),
                Err(_) => false,
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            // Device equality is the available boundary judgment on non-Linux
            // Unix; same-device nested mounts are not distinguishable here.
            let _ = target;
            true
        }
    }

    #[cfg(windows)]
    fn boundary_holds_impl(&self, _target: &Path, metadata: &fs::Metadata) -> bool {
        use std::os::windows::fs::MetadataExt;

        // A nested mounted volume or reparse target resolves to a different
        // volume serial; an undeterminable serial fails closed. Create passes
        // its canonical parent's metadata, so the same equality covers it.
        let root_volume = fs::metadata(&self.root)
            .ok()
            .and_then(|root| root.volume_serial_number());
        let target_volume = metadata.volume_serial_number();
        matches!((root_volume, target_volume), (Some(left), Some(right)) if left == right)
    }
}

/// Reads the Linux mount table's mount points, decoded.
#[cfg(target_os = "linux")]
fn linux_mount_points() -> Result<Vec<PathBuf>, std::io::Error> {
    let content = fs::read_to_string("/proc/self/mountinfo")?;
    Ok(content.lines().filter_map(parse_mount_point).collect())
}

/// One mountinfo line's mount point (field 5) as a path.
///
/// The field is the mount point in the mount namespace; `\040`, `\011`,
/// `\012`, and `\134` are the kernel's space/tab/newline/backslash escapes.
#[cfg(target_os = "linux")]
fn parse_mount_point(line: &str) -> Option<PathBuf> {
    let field = line.split_whitespace().nth(4)?;
    if field.is_empty() {
        return None;
    }
    Some(PathBuf::from(decode_mountinfo_escape(field)))
}

#[cfg(target_os = "linux")]
fn decode_mountinfo_escape(field: &str) -> String {
    let mut decoded = field.to_owned();
    for (escape, character) in [
        ("\\040", " "),
        ("\\011", "\t"),
        ("\\012", "\n"),
        ("\\134", "\\"),
    ] {
        decoded = decoded.replace(escape, character);
    }
    decoded
}

/// True when a mount point strictly below `root` sits on the `target` path.
///
/// The root itself may be a mount point; mount points above the root are
/// outside the workspace's own boundary.
#[cfg(target_os = "linux")]
fn crosses_linux_mount(root: &Path, target: &Path, mounts: &[PathBuf]) -> bool {
    mounts
        .iter()
        .any(|mount| mount != root && mount.starts_with(root) && target.starts_with(mount))
}

/// One observed execution result.
///
/// `output` carries read bytes or a listing and is redacted from
/// [`core::fmt::Debug`] so diagnostic output never leaks file content or
/// private names.
#[derive(Clone, PartialEq, Eq)]
pub struct ObservedEffect {
    pub certainty: ActionCertainty,
    pub grounds: EffectGrounds,
    pub output: Option<ActionOutput>,
}

impl core::fmt::Debug for ObservedEffect {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ObservedEffect")
            .field("certainty", &self.certainty)
            .field("grounds", &self.grounds)
            .field(
                "output",
                &self.output.as_ref().map(|output| match output {
                    ActionOutput::Bytes(bytes) if bytes.is_empty() => String::from("<0 bytes>"),
                    ActionOutput::Bytes(bytes) => format!("<{} bytes redacted>", bytes.len()),
                    ActionOutput::Listing(entries) => {
                        format!("<{} entries redacted>", entries.len())
                    }
                }),
            )
            .finish()
    }
}

fn refused() -> ObservedEffect {
    ObservedEffect {
        certainty: ActionCertainty::ConfirmedFailure,
        grounds: EffectGrounds::RefusedBeforeEffect,
        output: None,
    }
}

fn canonical_target(path: PathBuf) -> RealTargetRef {
    RealTargetRef::from_canonical_path(path.to_string_lossy().into_owned())
}

fn map_io_error(error: &std::io::Error) -> TargetRejection {
    match error.kind() {
        std::io::ErrorKind::NotFound => TargetRejection::MissingTarget,
        _ => TargetRejection::TargetUnavailable,
    }
}

/// Splits one requested path into normal component names.
///
/// Everything that is not a non-empty normal component is malformed: `..`,
/// absolute roots and prefixes, `.`, and empty names are all refused, so a
/// traversal never reaches the filesystem.
fn requested_components(requested: &str) -> Option<Vec<String>> {
    let mut names = Vec::new();
    for component in Path::new(requested).components() {
        match component {
            Component::Normal(name) => {
                let name = name.to_str()?;
                if name.is_empty() {
                    return None;
                }
                names.push(name.to_owned());
            }
            Component::RootDir
            | Component::Prefix(_)
            | Component::ParentDir
            | Component::CurDir => {
                return None;
            }
        }
    }
    if names.is_empty() { None } else { Some(names) }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{
        ActionOutput, ListEntry, ListEntryKind, MAX_ACTION_FILE_BYTES, TargetRejection,
        WorkspaceRoot,
    };
    use crate::attempt::OperationKind;

    fn workspace() -> (tempfile::TempDir, WorkspaceRoot) {
        let directory = tempdir().expect("test workspace directory");
        let root = WorkspaceRoot::open(&directory.path().to_string_lossy())
            .expect("an existing directory opens");
        (directory, root)
    }

    #[test]
    fn open_refuses_missing_and_non_directory_folders() {
        assert!(WorkspaceRoot::open("/nonexistent/ene/workspace").is_err());
        let directory = tempdir().expect("temporary directory");
        let file = directory.path().join("a.txt");
        fs::write(&file, b"x").expect("fixture write");
        assert!(
            WorkspaceRoot::open(&file.to_string_lossy()).is_err(),
            "a regular file is not a workspace folder"
        );
    }

    #[test]
    fn traversal_and_absolute_requests_are_malformed() {
        let (_directory, root) = workspace();
        for requested in [
            "../escape.txt",
            "a/../../escape.txt",
            "/etc/passwd",
            "./a.txt",
            ".",
            "..",
        ] {
            assert_eq!(
                root.resolve(requested, OperationKind::Read),
                Err(TargetRejection::MalformedPath),
                "malformed request must never reach the filesystem: {requested}"
            );
        }
        assert_eq!(
            root.resolve("", OperationKind::Create),
            Err(TargetRejection::MalformedPath)
        );
    }

    #[test]
    fn read_and_edit_require_an_existing_regular_file() {
        let (directory, root) = workspace();
        fs::write(directory.path().join("input.txt"), b"hello").expect("fixture write");
        let resolved = root
            .resolve("input.txt", OperationKind::Read)
            .expect("an existing file resolves");
        assert!(resolved.as_path().ends_with("input.txt"));
        assert_eq!(
            root.resolve("missing.txt", OperationKind::Read),
            Err(TargetRejection::MissingTarget)
        );
        fs::create_dir(directory.path().join("sub")).expect("fixture directory");
        assert_eq!(
            root.resolve("sub", OperationKind::Read),
            Err(TargetRejection::NotAFile)
        );
        assert_eq!(
            root.resolve("sub", OperationKind::Edit),
            Err(TargetRejection::NotAFile)
        );
    }

    #[test]
    fn read_refuses_targets_over_the_bound() {
        let (directory, root) = workspace();
        let bytes = vec![b'x'; MAX_ACTION_FILE_BYTES + 1];
        fs::write(directory.path().join("large.bin"), &bytes).expect("fixture write");
        assert_eq!(
            root.resolve("large.bin", OperationKind::Read),
            Err(TargetRejection::TooLarge)
        );
    }

    #[test]
    fn create_requires_a_contained_existing_parent_and_an_absent_destination() {
        let (directory, root) = workspace();
        let resolved = root
            .resolve("report.md", OperationKind::Create)
            .expect("a fresh file in the root resolves");
        let expected = fs::canonicalize(directory.path())
            .expect("canonical root")
            .join("report.md");
        assert_eq!(resolved.as_path(), expected.to_string_lossy());
        assert_eq!(
            root.resolve("missing/report.md", OperationKind::Create),
            Err(TargetRejection::MissingParent)
        );
        fs::write(directory.path().join("exists.md"), b"old").expect("fixture write");
        assert_eq!(
            root.resolve("exists.md", OperationKind::Create),
            Err(TargetRejection::AlreadyExists)
        );
        fs::create_dir(directory.path().join("sub")).expect("fixture directory");
        assert!(
            root.resolve("sub/report.md", OperationKind::Create).is_ok(),
            "an existing subdirectory is a valid parent"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_that_leave_the_workspace_are_refused() {
        let (directory, root) = workspace();
        let outside = tempdir().expect("outside directory");
        let outside_file = outside.path().join("secret.txt");
        fs::write(&outside_file, b"secret").expect("outside fixture");
        std::os::unix::fs::symlink(&outside_file, directory.path().join("escape.txt"))
            .expect("escape symlink");
        assert_eq!(
            root.resolve("escape.txt", OperationKind::Read),
            Err(TargetRejection::OutsideWorkspace)
        );
        std::os::unix::fs::symlink(outside.path(), directory.path().join("escape-dir"))
            .expect("escape directory symlink");
        assert_eq!(
            root.resolve("escape-dir/secret.txt", OperationKind::Read),
            Err(TargetRejection::OutsideWorkspace)
        );
        // A symlink whose target stays inside the folder is resolved inside.
        fs::write(directory.path().join("inside.txt"), b"inside").expect("inside fixture");
        std::os::unix::fs::symlink(
            directory.path().join("inside.txt"),
            directory.path().join("link.txt"),
        )
        .expect("inside symlink");
        let resolved = root
            .resolve("link.txt", OperationKind::Read)
            .expect("an inside symlink resolves to its contained target");
        assert!(resolved.as_path().ends_with("inside.txt"));
    }

    #[test]
    fn execute_reads_and_writes_with_honest_effects() {
        let (directory, root) = workspace();
        fs::write(directory.path().join("input.txt"), b"hello").expect("fixture write");

        let read_target = root
            .resolve("input.txt", OperationKind::Read)
            .expect("read target");
        let read = root.execute(&read_target, OperationKind::Read, None);
        assert_eq!(read.output, Some(ActionOutput::Bytes(b"hello".to_vec())));
        assert!(crate::attempt::certainty_grounds_pair_is_valid(
            read.certainty,
            read.grounds
        ));

        let create_target = root
            .resolve("report.md", OperationKind::Create)
            .expect("create target");
        let created = root.execute(&create_target, OperationKind::Create, Some(b"# report"));
        assert_eq!(
            created.certainty,
            crate::attempt::ActionCertainty::ConfirmedSuccess
        );
        assert_eq!(
            fs::read(directory.path().join("report.md")).expect("created file"),
            b"# report"
        );

        // Creating over an existing destination is refused at resolution.
        assert_eq!(
            root.resolve("input.txt", OperationKind::Create),
            Err(TargetRejection::AlreadyExists)
        );
        // A destination that appears after resolution is refused at publish
        // time and is never clobbered.
        let race_target = root
            .resolve("race.md", OperationKind::Create)
            .expect("a fresh target resolves");
        fs::write(directory.path().join("race.md"), b"winner").expect("race fixture");
        let raced = root.execute(&race_target, OperationKind::Create, Some(b"loser"));
        assert_eq!(
            raced.certainty,
            crate::attempt::ActionCertainty::ConfirmedFailure
        );
        assert_eq!(
            fs::read(directory.path().join("race.md")).expect("race file kept"),
            b"winner"
        );

        let edit_target = root
            .resolve("report.md", OperationKind::Edit)
            .expect("edit target");
        let edited = root.execute(&edit_target, OperationKind::Edit, Some(b"# edited"));
        assert_eq!(
            edited.certainty,
            crate::attempt::ActionCertainty::ConfirmedSuccess
        );
        assert_eq!(
            fs::read(directory.path().join("report.md")).expect("edited file"),
            b"# edited"
        );
    }

    #[test]
    fn execute_maps_missing_reads_to_confirmed_failure_without_an_effect() {
        let (_directory, root) = workspace();
        let target = crate::attempt::RealTargetRef::from_canonical_path(
            root.as_path()
                .join("missing.txt")
                .to_string_lossy()
                .into_owned(),
        );
        let effect = root.execute(&target, OperationKind::Read, None);
        assert_eq!(
            effect.certainty,
            crate::attempt::ActionCertainty::ConfirmedFailure
        );
        assert_eq!(
            effect.grounds,
            crate::attempt::EffectGrounds::RefusedBeforeEffect
        );
        assert_eq!(effect.output, None);
    }

    #[test]
    fn execute_refuses_a_read_that_grew_over_the_bound_at_effect_time() {
        let (directory, root) = workspace();
        let path = directory.path().join("large.bin");
        fs::write(&path, vec![b'x'; MAX_ACTION_FILE_BYTES + 1]).expect("fixture write");
        let target = crate::attempt::RealTargetRef::from_canonical_path(
            fs::canonicalize(&path)
                .expect("canonical fixture")
                .to_string_lossy()
                .into_owned(),
        );
        let effect = root.execute(&target, OperationKind::Read, None);
        assert_eq!(
            effect.certainty,
            crate::attempt::ActionCertainty::ConfirmedFailure
        );
        assert_eq!(effect.output, None);
    }

    #[test]
    fn forged_outside_targets_cannot_read_or_list_at_effect_time() {
        let (_directory, root) = workspace();
        let outside = tempdir().expect("outside directory");
        let outside_file = outside.path().join("secret.txt");
        fs::write(&outside_file, b"secret").expect("outside fixture");
        let canonical_file = fs::canonicalize(&outside_file).expect("canonical outside file");
        // A forged reference to an outside absolute path, even on the same
        // device, must fail closed at effect time.
        let forged_file = crate::attempt::RealTargetRef::from_canonical_path(
            canonical_file.to_string_lossy().into_owned(),
        );
        let read = root.execute(&forged_file, OperationKind::Read, None);
        assert_eq!(
            read.certainty,
            crate::attempt::ActionCertainty::ConfirmedFailure,
            "a forged outside target must not be readable"
        );
        assert_eq!(
            read.grounds,
            crate::attempt::EffectGrounds::RefusedBeforeEffect
        );
        assert_eq!(read.output, None);

        let canonical_dir = fs::canonicalize(outside.path()).expect("canonical outside directory");
        let forged_dir = crate::attempt::RealTargetRef::from_canonical_path(
            canonical_dir.to_string_lossy().into_owned(),
        );
        let listed = root.execute(&forged_dir, OperationKind::List, None);
        assert_eq!(
            listed.certainty,
            crate::attempt::ActionCertainty::ConfirmedFailure,
            "a forged outside directory must not be listable"
        );
        assert_eq!(listed.output, None);
    }

    #[test]
    fn debug_redacts_read_output() {
        let effect = super::ObservedEffect {
            certainty: crate::attempt::ActionCertainty::ConfirmedSuccess,
            grounds: crate::attempt::EffectGrounds::ObservedAtTarget,
            output: Some(ActionOutput::Bytes(b"secret file body".to_vec())),
        };
        let rendered = format!("{effect:?}");
        assert!(!rendered.contains("secret file body"));
        assert!(rendered.contains("bytes redacted"));
    }

    #[test]
    fn debug_redacts_listing_names() {
        let effect = super::ObservedEffect {
            certainty: crate::attempt::ActionCertainty::ConfirmedSuccess,
            grounds: crate::attempt::EffectGrounds::ObservedAtTarget,
            output: Some(ActionOutput::Listing(vec![ListEntry {
                name: String::from("private-notes.md"),
                kind: ListEntryKind::File,
            }])),
        };
        let rendered = format!("{effect:?}");
        assert!(!rendered.contains("private-notes.md"));
        assert!(rendered.contains("entries redacted"));
    }

    #[test]
    fn list_requires_a_directory_and_observes_a_sorted_listing() {
        let (directory, root) = workspace();
        fs::write(directory.path().join("b.txt"), b"b").expect("fixture write");
        fs::write(directory.path().join("a.txt"), b"a").expect("fixture write");
        fs::create_dir(directory.path().join("sub")).expect("fixture directory");
        assert_eq!(
            root.resolve("a.txt", OperationKind::List),
            Err(TargetRejection::NotADirectory)
        );
        assert_eq!(
            root.resolve("missing", OperationKind::List),
            Err(TargetRejection::MissingTarget)
        );
        let target = root
            .resolve("", OperationKind::List)
            .expect("an empty request lists the workspace root");
        assert_eq!(target.as_path(), root.as_path().to_string_lossy());
        let effect = root.execute(&target, OperationKind::List, None);
        assert_eq!(
            effect.certainty,
            crate::attempt::ActionCertainty::ConfirmedSuccess
        );
        assert_eq!(
            effect.output,
            Some(ActionOutput::Listing(vec![
                ListEntry {
                    name: String::from("a.txt"),
                    kind: ListEntryKind::File,
                },
                ListEntry {
                    name: String::from("b.txt"),
                    kind: ListEntryKind::File,
                },
                ListEntry {
                    name: String::from("sub"),
                    kind: ListEntryKind::Directory,
                },
            ]))
        );
    }

    #[cfg(unix)]
    #[test]
    fn list_excludes_symlink_entries_without_following_them() {
        let (directory, root) = workspace();
        let outside = tempdir().expect("outside directory");
        fs::write(outside.path().join("secret.txt"), b"secret").expect("outside fixture");
        std::os::unix::fs::symlink(outside.path(), directory.path().join("escape"))
            .expect("escape symlink");
        std::os::unix::fs::symlink(
            directory.path().join("inside"),
            directory.path().join("dangling"),
        )
        .expect("dangling symlink");
        let target = root
            .resolve(".", OperationKind::List)
            .expect("root listing");
        let effect = root.execute(&target, OperationKind::List, None);
        assert_eq!(
            effect.certainty,
            crate::attempt::ActionCertainty::ConfirmedSuccess
        );
        assert_eq!(
            effect.output,
            Some(ActionOutput::Listing(Vec::new())),
            "symlink entries are excluded, never followed or listed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn list_excludes_special_files() {
        let (directory, root) = workspace();
        fs::write(directory.path().join("regular.txt"), b"x").expect("fixture file");
        let _socket = std::os::unix::net::UnixListener::bind(directory.path().join("sock"))
            .expect("fixture socket");
        let target = root
            .resolve(".", OperationKind::List)
            .expect("root listing");
        let effect = root.execute(&target, OperationKind::List, None);
        assert_eq!(
            effect.output,
            Some(ActionOutput::Listing(vec![ListEntry {
                name: String::from("regular.txt"),
                kind: ListEntryKind::File,
            }]))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_mount_boundary_rejects_nested_mount_points() {
        use super::crosses_linux_mount;

        let root = PathBuf::from("/srv/workspace");
        let mounts = vec![
            PathBuf::from("/srv"),
            PathBuf::from("/srv/workspace/nested"),
        ];
        assert!(!crosses_linux_mount(
            &root,
            &PathBuf::from("/srv/workspace"),
            &mounts
        ));
        assert!(!crosses_linux_mount(
            &root,
            &PathBuf::from("/srv/workspace/file"),
            &mounts
        ));
        assert!(crosses_linux_mount(
            &root,
            &PathBuf::from("/srv/workspace/nested"),
            &mounts
        ));
        assert!(crosses_linux_mount(
            &root,
            &PathBuf::from("/srv/workspace/nested/file"),
            &mounts
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_mountinfo_lines_decode_kernel_escapes() {
        use super::{decode_mountinfo_escape, parse_mount_point};

        let line = "36 35 98:0 /mnt1 /srv/my\\040workspace rw,noatime master:1 - ext3 /dev/root rw,errors=continue";
        assert_eq!(
            parse_mount_point(line),
            Some(PathBuf::from("/srv/my workspace"))
        );
        assert_eq!(decode_mountinfo_escape("/a\\134b"), "/a\\b");
        assert_eq!(parse_mount_point("too short"), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_inside_targets_share_the_root_volume_serial() {
        use std::os::windows::fs::MetadataExt;

        let (directory, root) = workspace();
        fs::write(directory.path().join("input.txt"), b"hello").expect("fixture write");
        let canonical =
            fs::canonicalize(directory.path().join("input.txt")).expect("canonical fixture");
        let target_metadata = fs::metadata(&canonical).expect("target metadata");
        let root_metadata = fs::metadata(root.as_path()).expect("root metadata");
        // A nested mounted volume presents a different volume serial, so the
        // same equality refuses it; an undeterminable serial (None) fails
        // closed by the matches! guard in boundary_holds_impl.
        assert_eq!(
            root_metadata.volume_serial_number(),
            target_metadata.volume_serial_number(),
            "an inside target must share the root volume serial"
        );
        assert!(
            root.boundary_holds(&canonical, &target_metadata),
            "a same-volume inside target holds the boundary"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_reparse_points_are_excluded_and_escapes_refused() {
        use std::os::windows::fs::{symlink_dir, symlink_file};

        let (directory, root) = workspace();
        let outside = tempdir().expect("outside directory");
        fs::write(outside.path().join("secret.txt"), b"secret").expect("outside fixture");
        fs::write(directory.path().join("regular.txt"), b"x").expect("fixture file");
        // File and directory symlinks are reparse points, like junctions and
        // mounted volumes; creation may require privileges, so skip when the
        // platform refuses to create them.
        if symlink_file(
            outside.path().join("secret.txt"),
            directory.path().join("escape.txt"),
        )
        .is_err()
        {
            return;
        }
        // A junction-like directory reparse; a failure leaves only the file
        // reparse for the exclusion assertion below.
        let _ = symlink_dir(outside.path(), directory.path().join("escape-dir"));
        assert!(
            matches!(
                root.resolve("escape.txt", OperationKind::Read),
                Err(TargetRejection::OutsideWorkspace | TargetRejection::CrossFilesystem)
            ),
            "a reparse escape must not resolve inside the workspace"
        );
        let target = root.resolve("", OperationKind::List).expect("root listing");
        let effect = root.execute(&target, OperationKind::List, None);
        assert_eq!(
            effect.certainty,
            crate::attempt::ActionCertainty::ConfirmedSuccess
        );
        let Some(ActionOutput::Listing(entries)) = effect.output else {
            panic!("a listing observes entries");
        };
        assert!(
            entries
                .iter()
                .all(|entry| entry.name != "escape.txt" && entry.name != "escape-dir"),
            "reparse/junction entries are excluded, never followed: {entries:?}"
        );
        assert!(
            entries.iter().any(|entry| entry.name == "regular.txt"),
            "regular files remain listable: {entries:?}"
        );
    }
}
