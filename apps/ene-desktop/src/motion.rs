use std::path::{Path, PathBuf};

use ene_body::ipc::PoseClip;
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

#[must_use]
pub fn resolve(env: &MotionEnvironment) -> Vec<PoseClip> {
    if let Some(dir) = &env.motion_dir {
        return pose_clips_in(dir);
    }
    search_dirs(env)
        .into_iter()
        .map(|dir| pose_clips_in(&dir))
        .find(|clips| !clips.is_empty())
        .unwrap_or_default()
}

#[must_use]
pub fn install_roots(env: &MotionEnvironment) -> Vec<PathBuf> {
    let mut roots = vec![env.data_dir.clone()];
    if let Some(exe_dir) = &env.exe_dir {
        roots.push(exe_dir.clone());
        if let Some(prefix) = exe_dir.parent() {
            roots.push(prefix.join("share/ene"));
        }
    }
    if let Some(workspace) = &env.workspace_dir {
        roots.push(workspace.clone());
    }
    roots
}

fn search_dirs(env: &MotionEnvironment) -> Vec<PathBuf> {
    install_roots(env)
        .into_iter()
        .map(|root| root.join(BUNDLED_MOTION_DIR))
        .collect()
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|dir| !dir.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::{BUNDLED_MOTION_DIR, MotionEnvironment, resolve};
    use ene_body::ipc::PoseHint;
    use ene_body::testing::{MotionFixture, write_generated_vrma};
    use std::path::{Path, PathBuf};

    fn environment(dir: &Path) -> MotionEnvironment {
        MotionEnvironment {
            motion_dir: None,
            data_dir: dir.join("data"),
            exe_dir: Some(dir.join("bin")),
            workspace_dir: Some(dir.join("workspace")),
        }
    }

    fn write_clip(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        write_generated_vrma(
            &path,
            MotionFixture {
                bone: "head",
                yaw_degrees: 30.0,
                duration_secs: 0.5,
            },
        )
        .expect("clip fixture");
        path
    }

    #[test]
    fn explicit_override_is_used_without_searching() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut environment = environment(dir.path());
        let override_dir = dir.path().join("override");
        write_clip(&override_dir, "VRMA_06.vrma");
        write_clip(
            &environment.data_dir.join(BUNDLED_MOTION_DIR),
            "VRMA_01.vrma",
        );
        environment.motion_dir = Some(override_dir.clone());

        let clips = resolve(&environment);
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].pose, PoseHint::Idle);
    }

    #[test]
    fn the_data_directory_is_searched_before_the_installation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let environment = environment(dir.path());
        let data_motions = environment.data_dir.join(BUNDLED_MOTION_DIR);
        write_clip(&data_motions, "VRMA_01.vrma");
        let exe_dir = environment.exe_dir.clone().expect("exe dir");
        write_clip(&exe_dir.join(BUNDLED_MOTION_DIR), "VRMA_06.vrma");

        let clips = resolve(&environment);
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].pose, PoseHint::Speaking);
    }

    #[test]
    fn the_workspace_assets_are_used_for_development_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let environment = environment(dir.path());
        let workspace = environment.workspace_dir.clone().expect("workspace");
        write_clip(&workspace.join(BUNDLED_MOTION_DIR), "VRMA_06.vrma");

        let clips = resolve(&environment);
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].pose, PoseHint::Idle);
    }

    #[test]
    fn a_partial_pack_assigns_only_the_clips_it_has() {
        let dir = tempfile::tempdir().expect("tempdir");
        let environment = environment(dir.path());
        write_clip(
            &environment.data_dir.join(BUNDLED_MOTION_DIR),
            "VRMA_02.vrma",
        );

        let clips = resolve(&environment);
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].pose, PoseHint::Listening);
    }

    #[test]
    fn nothing_placed_yields_no_assignment() {
        let dir = tempfile::tempdir().expect("tempdir");
        let environment = environment(dir.path());

        let clips = resolve(&environment);
        assert!(clips.is_empty());
        assert!(!environment.data_dir.join(BUNDLED_MOTION_DIR).exists());
    }
}
