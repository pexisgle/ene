use std::path::Path;

use crate::ipc::{PoseClip, PoseHint};

pub const PACK_CLIP_FILES: [&str; 7] = [
    "VRMA_01.vrma",
    "VRMA_02.vrma",
    "VRMA_03.vrma",
    "VRMA_04.vrma",
    "VRMA_05.vrma",
    "VRMA_06.vrma",
    "VRMA_07.vrma",
];

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
