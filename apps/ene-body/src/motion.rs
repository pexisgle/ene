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
