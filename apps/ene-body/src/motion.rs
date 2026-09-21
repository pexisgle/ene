//! Bundled motion pack layout.
//!
//! The VRoid `VRMA_MotionPack` ships seven clips; the first-party character
//! uses five of them. These file names are asset-layout facts shared by the
//! desktop's resolver and the runtime probe. The body itself stays generic: it
//! plays whatever clip a parent assigns to a hint and never searches for one.

use std::path::Path;

use crate::ipc::{PoseClip, PoseHint};

/// Every clip file name the bundled pack ships, in pack order.
///
/// `VRMA_04` (shoot) and `VRMA_05` (spin) have no activity hint today; they are
/// still placed so a later hint needs no protocol change.
pub const PACK_CLIP_FILES: [&str; 7] = [
    "VRMA_01.vrma",
    "VRMA_02.vrma",
    "VRMA_03.vrma",
    "VRMA_04.vrma",
    "VRMA_05.vrma",
    "VRMA_06.vrma",
    "VRMA_07.vrma",
];

/// Activity hint → clip file name.
///
/// Idle uses the model pose, listening the greeting, speaking the full-body
/// presentation, working the stretching motion, and attention the V sign.
pub const DEFAULT_POSE_CLIPS: [(PoseHint, &str); 5] = [
    (PoseHint::Idle, "VRMA_06.vrma"),
    (PoseHint::Listening, "VRMA_02.vrma"),
    (PoseHint::Speaking, "VRMA_01.vrma"),
    (PoseHint::Working, "VRMA_07.vrma"),
    (PoseHint::Attention, "VRMA_03.vrma"),
];

/// Assigns the documented clips that exist in `dir` to their hints.
///
/// A clip that is absent simply has no assignment, so a partial pack degrades
/// per hint instead of failing the whole character. Callers own the search
/// order and any placement; this only reads the layout.
#[must_use]
pub fn pose_clips_in(dir: &Path) -> Vec<PoseClip> {
    DEFAULT_POSE_CLIPS
        .iter()
        .filter_map(|(pose, file)| {
            let candidate = dir.join(file);
            candidate.is_file().then(|| PoseClip {
                pose: *pose,
                path: candidate.to_string_lossy().into_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_POSE_CLIPS, pose_clips_in};
    use crate::ipc::PoseHint;
    use crate::testing::{MotionFixture, write_generated_vrma};

    #[test]
    fn only_the_documented_clips_are_assigned() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fixture = MotionFixture {
            bone: "head",
            yaw_degrees: 30.0,
            duration_secs: 0.5,
        };
        write_generated_vrma(&dir.path().join("VRMA_06.vrma"), fixture).expect("idle clip");
        write_generated_vrma(&dir.path().join("VRMA_02.vrma"), fixture).expect("greeting clip");
        // Unassigned pack members are placed but never mapped to a hint.
        write_generated_vrma(&dir.path().join("VRMA_04.vrma"), fixture).expect("shoot clip");

        let clips = pose_clips_in(dir.path());
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].pose, PoseHint::Idle);
        assert_eq!(clips[1].pose, PoseHint::Listening);
        assert!(clips[0].path.ends_with("VRMA_06.vrma"));
        assert_eq!(DEFAULT_POSE_CLIPS.len(), 5);
    }

    #[test]
    fn a_directory_without_pack_clips_assigns_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("readme.txt"), b"not a clip").expect("other file");
        assert!(pose_clips_in(dir.path()).is_empty());
        assert!(pose_clips_in(&dir.path().join("absent")).is_empty());
    }
}
