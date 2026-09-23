use std::path::Path;

use crate::ipc::{PoseClip, PoseHint};

pub const DEFAULT_POSE_CLIPS: [(PoseHint, &str); 5] = [
    (PoseHint::Idle, "VRMA_06.vrma"),
    (PoseHint::Listening, "VRMA_02.vrma"),
    (PoseHint::Speaking, "VRMA_01.vrma"),
    (PoseHint::Working, "VRMA_07.vrma"),
    (PoseHint::Attention, "VRMA_03.vrma"),
];

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
