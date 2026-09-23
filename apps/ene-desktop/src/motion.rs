//! Motion pack resolution for the body overlay.
//!
//! The desktop owns where the bundled clips live and which clip backs which
//! activity hint; the body only plays what it is handed. The VRoid motion pack
//! is an install asset: it is not in this repository and this module never
//! redistributes it. `assets/README.md` documents how to place a copy in one of
//! the locations below; resolution only reads them.

use std::path::{Path, PathBuf};

use ene_body::ipc::{MotionSetInfo, PoseClip};
use ene_body::motion::pose_clips_in;

/// Install-asset directory for the bundled character's clips.
pub const BUNDLED_MOTION_DIR: &str = "assets/motions";

/// Process facts and environment overrides the resolver reads.
///
/// Tests build this directly instead of mutating process environment, so
/// resolution stays deterministic under parallel test execution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MotionEnvironment {
    /// `ENE_MOTION_DIR`: use this directory and nothing else.
    pub motion_dir: Option<PathBuf>,
    /// Data directory of the installed application.
    pub data_dir: PathBuf,
    /// Directory of the running executable, when known.
    pub exe_dir: Option<PathBuf>,
    /// Workspace root, for development runs from a checkout.
    pub workspace_dir: Option<PathBuf>,
}

impl MotionEnvironment {
    /// Reads the real process environment for `data_dir`.
    #[must_use]
    pub fn from_process(data_dir: &Path) -> Self {
        Self {
            motion_dir: env_dir("ENE_MOTION_DIR"),
            data_dir: data_dir.to_path_buf(),
            exe_dir: std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf)),
            workspace_dir: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")),
        }
    }
}

/// Where the clips were found and what can be assigned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MotionPlan {
    /// Directory that supplied the clips; `None` when none were found.
    pub source: Option<PathBuf>,
    /// Pose → clip assignment, in [`ene_body::motion::DEFAULT_POSE_CLIPS`]
    /// order.
    pub clips: Vec<PoseClip>,
}

impl MotionPlan {
    /// The projection to send, or `None` when no clip is available. A hint
    /// without a clip keeps the body's hand-authored staging.
    #[must_use]
    pub fn set(&self) -> Option<MotionSetInfo> {
        (!self.clips.is_empty()).then(|| MotionSetInfo {
            clips: self.clips.clone(),
        })
    }
}

/// Resolves the assignment from the standard locations, most specific first.
///
/// Placement is not this module's job: when nothing is placed, the body keeps
/// its hand-authored staging and `HealthTick.motion` reports `Unsupported`.
#[must_use]
pub fn resolve(env: &MotionEnvironment) -> MotionPlan {
    if let Some(dir) = &env.motion_dir {
        // An explicit override is used exactly as given: no search.
        return plan_from_dir(dir);
    }
    search_dirs(env)
        .into_iter()
        .map(|dir| plan_from_dir(&dir))
        .find(|plan| !plan.clips.is_empty())
        .unwrap_or(MotionPlan {
            source: None,
            clips: Vec::new(),
        })
}

/// Locations holding install assets, most specific first.
fn search_dirs(env: &MotionEnvironment) -> Vec<PathBuf> {
    let mut dirs = vec![env.data_dir.join(BUNDLED_MOTION_DIR)];
    if let Some(exe_dir) = &env.exe_dir {
        dirs.push(exe_dir.join(BUNDLED_MOTION_DIR));
        if let Some(prefix) = exe_dir.parent() {
            dirs.push(prefix.join("share/ene").join(BUNDLED_MOTION_DIR));
        }
    }
    if let Some(workspace) = &env.workspace_dir {
        dirs.push(workspace.join(BUNDLED_MOTION_DIR));
    }
    dirs
}

/// Builds a plan from one directory. Clips that are absent simply have no
/// assignment: a partial pack degrades per hint instead of failing the body.
fn plan_from_dir(dir: &Path) -> MotionPlan {
    let clips = pose_clips_in(dir);
    MotionPlan {
        source: (!clips.is_empty()).then(|| dir.to_path_buf()),
        clips,
    }
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|dir| !dir.as_os_str().is_empty())
}
