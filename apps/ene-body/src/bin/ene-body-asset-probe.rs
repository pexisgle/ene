use std::path::PathBuf;
use std::process::ExitCode;

use ene_body::ipc::{AssetReadyInfo, AssetRef, FeatureSupport, MotionSetInfo, PoseHint};
use ene_body::motion::pose_clips_in;
use ene_body::vrm::VrmSession;
use serde::Serialize;

#[derive(Serialize)]
struct ProbeReport {
    asset: String,
    runtime: &'static str,
    runtime_version: &'static str,
    strict_vrm_1_load: bool,
    humanoid_pose_hints: bool,
    expressions: bool,
    look_at: bool,
    spring_bone: bool,
    renderer_frame_data: bool,
    motion_dir: Option<String>,
    vrma_motion_pack: bool,
    pose_motions: Vec<PoseHint>,
    stats: AssetReadyInfo,
    note: &'static str,
}

fn main() -> ExitCode {
    match run() {
        Ok(report) => match serde_json::to_writer_pretty(std::io::stdout(), &report) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("failed to encode probe report: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ProbeReport, String> {
    let mut args = std::env::args_os();
    let _program = args.next();
    let path = PathBuf::from(
        args.next()
            .ok_or_else(|| String::from("usage: ene-body-asset-probe PATH.vrm [MOTION_DIR]"))?,
    );
    let motion_dir = args.next().map(PathBuf::from);
    if args.next().is_some() {
        return Err(String::from(
            "usage: ene-body-asset-probe PATH.vrm [MOTION_DIR]",
        ));
    }
    let mut session = VrmSession::new();
    session
        .set_asset(AssetRef::Path {
            path: path.to_string_lossy().into_owned(),
        })
        .map_err(|error| format!("{:?}: {}", error.reason, error.detail))?;
    let stats = session
        .stats()
        .ok_or_else(|| String::from("runtime did not retain asset statistics"))?;
    let mut pose_motions = Vec::new();
    if let Some(dir) = motion_dir.as_deref() {
        session
            .set_motions(&MotionSetInfo {
                clips: pose_clips_in(dir),
            })
            .map_err(|error| std::format!("{:?}: {}", error.reason, error.detail))?;
        pose_motions = session.motion_poses();
    }
    let mut renderer_frame_data = true;
    for pose in [
        PoseHint::Idle,
        PoseHint::Listening,
        PoseHint::Speaking,
        PoseHint::Working,
        PoseHint::Attention,
    ] {
        session.set_pose(pose);
        for _ in 0..30 {
            let meshes = session
                .update(1.0 / 30.0)
                .map_err(|error| format!("{:?}: {}", error.reason, error.detail))?;
            renderer_frame_data &= !meshes.is_empty()
                && meshes.iter().all(|mesh| {
                    !mesh.mesh.positions.is_empty()
                        && mesh
                            .mesh
                            .positions
                            .iter()
                            .flatten()
                            .all(|component| component.is_finite())
                });
        }
    }
    if !renderer_frame_data {
        return Err(String::from(
            "runtime produced empty or non-finite renderer frame data",
        ));
    }
    Ok(ProbeReport {
        asset: path.display().to_string(),
        runtime: "vrm-runtime",
        runtime_version: "0.1.0",
        strict_vrm_1_load: true,
        humanoid_pose_hints: true,
        expressions: stats.expressions > 0,
        look_at: true,
        spring_bone: stats.spring_chains > 0,
        renderer_frame_data,
        motion_dir: motion_dir.map(|dir| dir.display().to_string()),
        vrma_motion_pack: session.motion() == FeatureSupport::Available,
        pose_motions,
        stats,
        note: "runtime probe only; real compositor and official ene acceptance are separate",
    })
}
