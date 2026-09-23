use std::path::{Path, PathBuf};

use ene_body::ipc::{MotionSetInfo, PoseClip};
use ene_body::motion::pose_clips_in;

pub const BUNDLED_MOTION_DIR: &str = "assets/motions";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MotionEnvironment {
    pub motion_dir: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub exe_dir: Option<PathBuf>,
    pub workspace_dir: Option<PathBuf>,
}

impl MotionEnvironment {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MotionPlan {
    pub source: Option<PathBuf>,
    pub clips: Vec<PoseClip>,
}

impl MotionPlan {
    #[must_use]
    pub fn set(&self) -> Option<MotionSetInfo> {
        (!self.clips.is_empty()).then(|| MotionSetInfo {
            clips: self.clips.clone(),
        })
    }
}

#[must_use]
pub fn resolve(env: &MotionEnvironment) -> MotionPlan {
    if let Some(dir) = &env.motion_dir {
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
