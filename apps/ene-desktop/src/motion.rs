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
    use super::{BUNDLED_MOTION_DIR, MotionEnvironment, resolve, search_dirs};
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
    fn override_and_search_root_precedence_are_table_driven() {
        let cases = [
            (
                Some(("VRMA_06.vrma", PoseHint::Idle)),
                Some(("VRMA_01.vrma", PoseHint::Speaking)),
                Some(("VRMA_02.vrma", PoseHint::Listening)),
                Some(("VRMA_07.vrma", PoseHint::Working)),
                Some(PoseHint::Idle),
            ),
            (
                None,
                Some(("VRMA_01.vrma", PoseHint::Speaking)),
                Some(("VRMA_02.vrma", PoseHint::Listening)),
                Some(("VRMA_07.vrma", PoseHint::Working)),
                Some(PoseHint::Speaking),
            ),
            (
                None,
                None,
                Some(("VRMA_02.vrma", PoseHint::Listening)),
                Some(("VRMA_07.vrma", PoseHint::Working)),
                Some(PoseHint::Listening),
            ),
            (
                None,
                None,
                None,
                Some(("VRMA_06.vrma", PoseHint::Idle)),
                Some(PoseHint::Idle),
            ),
            (None, None, None, None, None),
        ];

        for (override_clip, data_clip, exe_clip, workspace_clip, expected) in cases {
            let dir = tempfile::tempdir().expect("tempdir");
            let mut environment = environment(dir.path());
            if let Some((name, _)) = override_clip {
                write_clip(&dir.path().join("override"), name);
                environment.motion_dir = Some(dir.path().join("override"));
            }
            if let Some((name, _)) = data_clip {
                write_clip(&environment.data_dir.join(BUNDLED_MOTION_DIR), name);
            }
            if let Some((name, _)) = exe_clip {
                write_clip(
                    &environment
                        .exe_dir
                        .as_ref()
                        .expect("exe dir")
                        .join(BUNDLED_MOTION_DIR),
                    name,
                );
            }
            if let Some((name, _)) = workspace_clip {
                write_clip(
                    &environment
                        .workspace_dir
                        .as_ref()
                        .expect("workspace dir")
                        .join(BUNDLED_MOTION_DIR),
                    name,
                );
            }

            let poses = resolve(&environment)
                .into_iter()
                .map(|clip| clip.pose)
                .collect::<Vec<_>>();
            assert_eq!(poses, expected.into_iter().collect::<Vec<_>>());
            if expected.is_none() {
                for root in search_dirs(&environment) {
                    assert!(!root.exists(), "resolution created {}", root.display());
                }
            }
        }
    }

    #[test]
    fn partial_packs_keep_their_mapped_clips_in_canonical_order() {
        type ClipEntry = (&'static str, PoseHint);
        type PackCase = (&'static [ClipEntry], &'static [PoseHint]);
        let cases: &[PackCase] = &[
            (
                &[
                    ("VRMA_03.vrma", PoseHint::Attention),
                    ("VRMA_01.vrma", PoseHint::Speaking),
                ],
                &[PoseHint::Speaking, PoseHint::Attention],
            ),
            (
                &[
                    ("VRMA_07.vrma", PoseHint::Working),
                    ("VRMA_06.vrma", PoseHint::Idle),
                    ("VRMA_02.vrma", PoseHint::Listening),
                ],
                &[PoseHint::Idle, PoseHint::Listening, PoseHint::Working],
            ),
        ];

        for (files, expected) in cases {
            let dir = tempfile::tempdir().expect("tempdir");
            let environment = environment(dir.path());
            let motions = environment.data_dir.join(BUNDLED_MOTION_DIR);
            for (name, _) in *files {
                write_clip(&motions, name);
            }

            let poses = resolve(&environment)
                .into_iter()
                .map(|clip| clip.pose)
                .collect::<Vec<_>>();
            assert_eq!(poses, *expected);
        }
    }
}
