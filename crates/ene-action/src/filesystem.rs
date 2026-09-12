//! The Workspace-contained filesystem execution boundary (E-1/E-2, K-H).
//!
//! A [`WorkspaceRoot`] is a canonicalized folder. [`WorkspaceRoot::resolve`]
//! turns one requested workspace-relative path into a [`RealTargetRef`]: a
//! canonical absolute path proven to be inside the folder at resolution time.
//! Absolute paths, parent-directory components, and symlink resolutions that
//! leave the folder are refused; string equality of the request is never
//! treated as identity.
//!
//! [`WorkspaceRoot::execute`] re-verifies containment immediately before the
//! effect and publishes writes atomically in the destination directory
//! (`persist_noclobber` for create, `persist` for edit), then reads the
//! destination back. Confirmed success means the executor observed the
//! intended content at the target; an agent self-report is never a ground.
//!
//! The guarantee is against the agent-requested path, not against an
//! unbounded concurrent local writer: a same-user process swapping directory
//! components mid-operation is outside the container the OS gives this
//! process, and an `openat2`-style syscall boundary is platform work this
//! slice does not claim.

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

/// Why one requested path was refused before any attempt was claimed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TargetRejection {
    /// Empty, non-UTF-8, or containing `..`/absolute/root components.
    #[error("requested path is malformed")]
    MalformedPath,
    /// The canonical resolution left the workspace folder (including symlinks).
    #[error("requested path escapes the workspace")]
    OutsideWorkspace,
    /// Read/edit named a target that does not exist.
    #[error("requested target does not exist")]
    MissingTarget,
    /// Create named a target whose parent directory does not exist.
    #[error("requested target parent does not exist")]
    MissingParent,
    /// The resolved target is not a regular file.
    #[error("requested target is not a regular file")]
    NotAFile,
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
    /// Read/edit require an existing regular file inside the folder; create
    /// requires an existing directory parent inside the folder and a
    /// destination that does not exist yet. The returned target is the
    /// canonical absolute path, which is what execution uses.
    pub fn resolve(
        &self,
        requested: &str,
        operation: OperationKind,
    ) -> Result<RealTargetRef, TargetRejection> {
        let names = requested_components(requested).ok_or(TargetRejection::MalformedPath)?;
        match operation {
            OperationKind::Read | OperationKind::Edit => {
                let mut joined = self.root.clone();
                for name in &names {
                    joined.push(name);
                }
                let canonical = fs::canonicalize(&joined).map_err(|error| map_io_error(&error))?;
                if !canonical.starts_with(&self.root) {
                    return Err(TargetRejection::OutsideWorkspace);
                }
                let metadata = fs::metadata(&canonical).map_err(|error| map_io_error(&error))?;
                if !metadata.is_file() {
                    return Err(TargetRejection::NotAFile);
                }
                if operation == OperationKind::Read && metadata.len() > MAX_ACTION_FILE_BYTES as u64
                {
                    return Err(TargetRejection::TooLarge);
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
    /// Reads return the observed bytes. Writes publish atomically in the
    /// destination directory and read the destination back; a verified
    /// read-back is the confirmed-success ground, a refusal before publishing
    /// is a confirmed failure, and anything the executor cannot verify stays
    /// [`ActionCertainty::Unknown`].
    ///
    /// `content` is required for create/edit and ignored for read; the
    /// orchestration checks this before any durable claim.
    #[must_use]
    pub fn execute(
        &self,
        target: &RealTargetRef,
        operation: OperationKind,
        content: Option<&[u8]>,
    ) -> ObservedEffect {
        match operation {
            OperationKind::Read => {
                // Re-check the bound at the effect moment: a file that grew
                // after resolution must not turn into an unbounded read.
                let too_large = fs::metadata(target.as_path())
                    .is_ok_and(|metadata| metadata.len() > MAX_ACTION_FILE_BYTES as u64);
                if too_large {
                    return refused();
                }
                match fs::read(target.as_path()) {
                    Ok(bytes) => ObservedEffect {
                        certainty: ActionCertainty::ConfirmedSuccess,
                        grounds: EffectGrounds::ObservedAtTarget,
                        output: Some(bytes),
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
    /// root; create requires the canonical parent to still be inside the root
    /// and the destination to still be absent.
    fn reverifies_at_effect(&self, destination: &Path, replace: bool) -> bool {
        if replace {
            match fs::canonicalize(destination) {
                Ok(canonical) => canonical == destination && canonical.starts_with(&self.root),
                Err(_) => false,
            }
        } else {
            let Some(parent) = destination.parent() else {
                return false;
            };
            match fs::canonicalize(parent) {
                Ok(canonical_parent) => {
                    canonical_parent == parent
                        && canonical_parent.starts_with(&self.root)
                        && fs::symlink_metadata(destination).is_err()
                }
                Err(_) => false,
            }
        }
    }
}

/// One observed execution result.
///
/// `output` carries the read bytes and is redacted from [`core::fmt::Debug`]
/// so diagnostic output never leaks file content.
#[derive(Clone, PartialEq, Eq)]
pub struct ObservedEffect {
    pub certainty: ActionCertainty,
    pub grounds: EffectGrounds,
    pub output: Option<Vec<u8>>,
}

impl core::fmt::Debug for ObservedEffect {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ObservedEffect")
            .field("certainty", &self.certainty)
            .field("grounds", &self.grounds)
            .field(
                "output",
                &self.output.as_ref().map(|bytes| {
                    if bytes.is_empty() {
                        String::from("<0 bytes>")
                    } else {
                        format!("<{} bytes redacted>", bytes.len())
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

    use tempfile::tempdir;

    use super::{MAX_ACTION_FILE_BYTES, TargetRejection, WorkspaceRoot};
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
        assert_eq!(read.output.as_deref(), Some(&b"hello"[..]));
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
    fn debug_redacts_read_output() {
        let effect = super::ObservedEffect {
            certainty: crate::attempt::ActionCertainty::ConfirmedSuccess,
            grounds: crate::attempt::EffectGrounds::ObservedAtTarget,
            output: Some(b"secret file body".to_vec()),
        };
        let rendered = format!("{effect:?}");
        assert!(!rendered.contains("secret file body"));
        assert!(rendered.contains("bytes redacted"));
    }
}
