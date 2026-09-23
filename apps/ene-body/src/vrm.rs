use crate::ipc::{
    AssetFailInfo, AssetFailReason, AssetReadyInfo, AssetRef, FeatureSupport, MotionFailInfo,
    MotionFailReason, MotionSetInfo, PoseHint,
};
use std::collections::BTreeSet;
use std::sync::Arc;

use vrm_runtime::{PlaybackMode, PlaybackOptions, VrmAnimation};

const POSE_ORDER: [PoseHint; 5] = [
    PoseHint::Idle,
    PoseHint::Listening,
    PoseHint::Speaking,
    PoseHint::Working,
    PoseHint::Attention,
];

const MOTION_PLAYBACK: PlaybackOptions = PlaybackOptions {
    speed: 1.0,
    mode: PlaybackMode::Loop,
    scale_hips_translation: true,
};

/// One CPU-deformed primitive plus its expression-evaluated PBR base color.
/// Base-color texture sampling and the evaluated base-color factor are
/// implemented; MToon shading terms (shade, matcap, rim, outline) remain
/// renderer quality work. Geometry, skinning, morphs, and vertex colors are
/// retained.
#[derive(Debug, Clone)]
pub struct RenderMesh {
    pub mesh: vrm_runtime::CpuMesh,
    pub base_color: [f32; 4],
    pub texture: Option<RenderTexture>,
    pub texcoords: Vec<[f32; 2]>,
}

#[derive(Debug, Clone)]
pub struct RenderTexture {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

#[derive(Debug, Clone)]
struct MaterialTexture {
    texture: RenderTexture,
    tex_coord: usize,
    transform: vrm_runtime::TextureTransform,
}

pub struct VrmSession {
    runtime: Option<vrm_runtime::AvatarRuntime>,
    motions: [Option<Arc<VrmAnimation>>; POSE_ORDER.len()],
    playing: Option<PoseHint>,
    pose: PoseHint,
    elapsed_secs: f32,
    stats: Option<AssetReadyInfo>,
    material_textures: Vec<Option<MaterialTexture>>,
    asset_generation: u32,
}

impl Default for VrmSession {
    fn default() -> Self {
        Self::new()
    }
}

impl VrmSession {
    #[must_use]
    pub fn new() -> Self {
        Self {
            runtime: None,
            motions: std::array::from_fn(|_| None),
            playing: None,
            pose: PoseHint::Idle,
            elapsed_secs: 0.0,
            stats: None,
            material_textures: Vec::new(),
            asset_generation: 0,
        }
    }

    #[must_use]
    pub fn stats(&self) -> Option<AssetReadyInfo> {
        self.stats
    }

    /// Changes the Body-local activity projection. Runtime controls are
    /// applied on the next fixed-rate update.
    pub fn set_pose(&mut self, pose: PoseHint) {
        self.pose = pose;
    }

    #[must_use]
    pub fn motion(&self) -> FeatureSupport {
        if self.motions.iter().any(Option::is_some) {
            FeatureSupport::Available
        } else {
            FeatureSupport::Unsupported
        }
    }

    #[must_use]
    pub fn motion_poses(&self) -> Vec<PoseHint> {
        POSE_ORDER
            .iter()
            .copied()
            .filter(|pose| {
                self.motions
                    .get(pose_index(*pose))
                    .is_some_and(Option::is_some)
            })
            .collect()
    }

    pub fn set_motions(&mut self, set: &MotionSetInfo) -> Result<(), MotionFailInfo> {
        set.validate()?;
        let mut loaded = Vec::with_capacity(set.clips.len());
        for clip in &set.clips {
            let path = clip.path.as_str();
            regular_file(path).map_err(|problem| match problem {
                PathProblem::Missing => MotionFailInfo::new(
                    MotionFailReason::Missing,
                    "motion clip file is unavailable",
                ),
                PathProblem::NotAFile => MotionFailInfo::new(
                    MotionFailReason::NotAFile,
                    "motion clip path is not a regular file",
                ),
            })?;
            let animation = VrmAnimation::load(path).map_err(|error| {
                MotionFailInfo::new(
                    MotionFailReason::InvalidVrma,
                    std::format!("VRMA validation failed: {error}"),
                )
            })?;
            loaded.push((clip.pose, Arc::new(animation)));
        }
        let mut motions: [Option<Arc<VrmAnimation>>; POSE_ORDER.len()] =
            std::array::from_fn(|_| None);
        for (pose, animation) in loaded {
            if let Some(slot) = motions.get_mut(pose_index(pose)) {
                *slot = Some(animation);
            }
        }
        self.motions = motions;
        self.playing = None;
        Ok(())
    }

    pub fn set_asset(&mut self, asset: AssetRef) -> Result<(), AssetFailInfo> {
        let path = asset.path();
        if path.is_empty() {
            return Err(fail(AssetFailReason::EmptyPath, "asset path is empty"));
        }
        regular_file(path).map_err(|problem| match problem {
            PathProblem::Missing => fail(AssetFailReason::Missing, "asset file is unavailable"),
            PathProblem::NotAFile => fail(
                AssetFailReason::NotAFile,
                "asset path is not a regular file",
            ),
        })?;
        let avatar = vrm_runtime::AvatarAsset::load(path).map_err(|error| {
            fail(
                AssetFailReason::InvalidVrm,
                format!("VRM 1.0 validation failed: {error}"),
            )
        })?;
        if avatar.primitives().is_empty() {
            return Err(fail(
                AssetFailReason::MissingPrimitives,
                "VRM has no renderer primitives",
            ));
        }
        if avatar.expressions().is_empty() {
            return Err(fail(
                AssetFailReason::MissingExpressions,
                "VRM has no expressions",
            ));
        }
        if avatar.look_at().is_none() {
            return Err(fail(
                AssetFailReason::MissingLookAt,
                "VRM has no LookAt definition",
            ));
        }
        if avatar.spring_bone().springs.is_empty() {
            return Err(fail(
                AssetFailReason::MissingSpringBone,
                "VRM has no SpringBone chains",
            ));
        }
        let stats = AssetReadyInfo {
            primitives: avatar.primitives().len(),
            expressions: avatar.expressions().len(),
            spring_chains: avatar.spring_bone().springs.len(),
        };
        let asset_generation = self.asset_generation.saturating_add(1);
        let material_textures = decode_material_textures(&avatar, asset_generation)?;
        let mut runtime = vrm_runtime::AvatarRuntime::new(avatar);
        runtime.update(0.0).map_err(|error| {
            fail(
                AssetFailReason::RuntimeEvaluation,
                format!("initial runtime evaluation failed: {error}"),
            )
        })?;
        self.runtime = Some(runtime);
        self.playing = None;
        self.elapsed_secs = 0.0;
        self.stats = Some(stats);
        self.material_textures = material_textures;
        self.asset_generation = asset_generation;
        Ok(())
    }

    pub fn update(&mut self, dt: f32) -> Result<Vec<RenderMesh>, AssetFailInfo> {
        let pose = self.pose;
        let clip = self
            .motions
            .get(pose_index(pose))
            .and_then(|slot| slot.clone());
        self.elapsed_secs = (self.elapsed_secs + dt).rem_euclid(120.0);
        let elapsed_secs = self.elapsed_secs;
        let Some(runtime) = self.runtime.as_mut() else {
            return Ok(Vec::new());
        };
        match clip {
            Some(clip) => {
                if self.playing != Some(pose) {
                    runtime.play(clip, MOTION_PLAYBACK).map_err(|error| {
                        fail(
                            AssetFailReason::RuntimeEvaluation,
                            std::format!("motion playback failed: {error}"),
                        )
                    })?;
                }
                apply_clip_staging(runtime, pose);
                self.playing = Some(pose);
            }
            None => {
                runtime.stop();
                self.playing = None;
                apply_procedural_pose(runtime, pose, elapsed_secs);
            }
        }
        let material_textures = &self.material_textures;
        runtime
            .update(dt)
            .map_err(|error| {
                fail(
                    AssetFailReason::RuntimeEvaluation,
                    format!("runtime evaluation failed: {error}"),
                )
            })
            .map(|frame| {
                let draws = frame
                    .gpu()
                    .draws()
                    .map(|draw| {
                        let base_color = draw
                            .evaluated_material
                            .map_or([1.0; 4], |material| material.base_color_factor);
                        let dynamic_transform = draw
                            .evaluated_material
                            .map_or_else(vrm_runtime::TextureTransform::default, |material| {
                                material.texture_transform
                            });
                        let uv_animation = draw
                            .evaluated_material
                            .map_or_else(vrm_runtime::UvAnimationState::default, |material| {
                                material.uv_animation
                            });
                        let texture = draw
                            .material_id
                            .and_then(|id| material_textures.get(id.index()))
                            .and_then(Option::as_ref)
                            .cloned();
                        (base_color, dynamic_transform, uv_animation, texture)
                    })
                    .collect::<Vec<_>>();
                frame
                    .bake_cpu()
                    .into_iter()
                    .zip(draws)
                    .map(
                        |(mesh, (base_color, dynamic_transform, uv_animation, texture))| {
                            let texcoords = if let Some(texture) = texture.as_ref() {
                                mesh.texcoord_sets
                                    .get(texture.tex_coord)
                                    .map(|coordinates| {
                                        coordinates
                                            .iter()
                                            .copied()
                                            .map(|uv| {
                                                transformed_uv(
                                                    transformed_uv(
                                                        animated_uv(uv, uv_animation),
                                                        dynamic_transform,
                                                    ),
                                                    texture.transform,
                                                )
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default()
                            } else {
                                Vec::new()
                            };
                            RenderMesh {
                                mesh,
                                base_color,
                                texture: texture.map(|texture| texture.texture),
                                texcoords,
                            }
                        },
                    )
                    .collect()
            })
    }
}

fn decode_material_textures(
    avatar: &vrm_runtime::AvatarAsset,
    asset_generation: u32,
) -> Result<Vec<Option<MaterialTexture>>, AssetFailInfo> {
    let needed: BTreeSet<usize> = avatar
        .materials()
        .iter()
        .filter_map(|material| {
            material
                .base_color_texture()
                .map(|info| info.texture_id().index())
        })
        .collect();
    let mut decoded = vec![None; avatar.textures().len()];
    for (id, texture) in avatar.textures().iter().enumerate() {
        if !needed.contains(&id) {
            continue;
        }
        let image = avatar.image(texture.image_id()).ok_or_else(|| {
            fail(
                AssetFailReason::InvalidVrm,
                "base-color texture references a missing image",
            )
        })?;
        let rgba = image::load_from_memory(image.data())
            .map_err(|error| {
                fail(
                    AssetFailReason::InvalidVrm,
                    format!("base-color image decode failed: {error}"),
                )
            })?
            .into_rgba8();
        let (width, height) = rgba.dimensions();
        decoded[id] = Some(RenderTexture {
            id: (u64::from(asset_generation) << 32) | u64::try_from(id).unwrap_or(u64::MAX),
            width,
            height,
            rgba: Arc::from(rgba.into_raw()),
        });
    }
    avatar
        .materials()
        .iter()
        .map(|material| {
            let Some(info) = material.base_color_texture() else {
                return Ok(None);
            };
            let texture = decoded
                .get(info.texture_id().index())
                .and_then(Option::as_ref)
                .cloned()
                .ok_or_else(|| {
                    fail(
                        AssetFailReason::InvalidVrm,
                        "base-color texture could not be decoded",
                    )
                })?;
            Ok(Some(MaterialTexture {
                texture,
                tex_coord: info.tex_coord() as usize,
                transform: info.transform(),
            }))
        })
        .collect()
}

fn affine_uv(uv: [f32; 2], offset: [f32; 2], scale: [f32; 2], rotation: f32) -> [f32; 2] {
    let scaled = [uv[0] * scale[0], uv[1] * scale[1]];
    let (sin, cos) = rotation.sin_cos();
    [
        offset[0] + cos * scaled[0] - sin * scaled[1],
        offset[1] + sin * scaled[0] + cos * scaled[1],
    ]
}

fn transformed_uv(uv: [f32; 2], transform: vrm_runtime::TextureTransform) -> [f32; 2] {
    affine_uv(uv, transform.offset, transform.scale, transform.rotation)
}

fn animated_uv(uv: [f32; 2], animation: vrm_runtime::UvAnimationState) -> [f32; 2] {
    affine_uv(uv, animation.scroll, [1.0, 1.0], animation.rotation)
}

fn fail(reason: AssetFailReason, detail: impl Into<String>) -> AssetFailInfo {
    AssetFailInfo {
        reason,
        detail: detail.into(),
    }
}

/// Filesystem problem shared by the asset and motion-clip preconditions. The
/// two callers map it onto their own failure reason type.
enum PathProblem {
    Missing,
    NotAFile,
}

fn regular_file(path: &str) -> Result<(), PathProblem> {
    let meta = std::fs::metadata(path).map_err(|_| PathProblem::Missing)?;
    if meta.is_file() {
        Ok(())
    } else {
        Err(PathProblem::NotAFile)
    }
}

/// Slot of a pose hint inside [`POSE_ORDER`], which is the single source of
/// the slot order.
fn pose_index(pose: PoseHint) -> usize {
    POSE_ORDER
        .iter()
        .position(|candidate| *candidate == pose)
        .unwrap_or(0)
}

struct Staging {
    expression: &'static str,
    weight: f32,
    look_at: [f32; 3],
    head_yaw: f32,
    spine_roll: f32,
}

fn staging(pose: PoseHint, phase: f32) -> Staging {
    match pose {
        PoseHint::Idle => Staging {
            expression: "relaxed",
            weight: 0.25,
            look_at: [0.0, 1.45, 2.0],
            head_yaw: phase.sin() * 0.025,
            spine_roll: 0.01,
        },
        PoseHint::Listening => Staging {
            expression: "relaxed",
            weight: 0.45,
            look_at: [0.2, 1.5, 1.8],
            head_yaw: -0.08,
            spine_roll: -0.02,
        },
        PoseHint::Speaking => Staging {
            expression: "aa",
            weight: 0.75,
            look_at: [0.0, 1.5, 2.0],
            head_yaw: phase.sin() * 0.06,
            spine_roll: 0.025,
        },
        PoseHint::Working => Staging {
            expression: "neutral",
            weight: 0.6,
            look_at: [0.0, 1.0, 1.4],
            head_yaw: 0.03,
            spine_roll: -0.035,
        },
        PoseHint::Attention => Staging {
            expression: "surprised",
            weight: 0.65,
            look_at: [0.0, 1.6, 1.5],
            head_yaw: 0.0,
            spine_roll: 0.0,
        },
    }
}

fn apply_expression(runtime: &mut vrm_runtime::AvatarRuntime, staging: &Staging) {
    if runtime
        .asset()
        .expressions()
        .get(staging.expression)
        .is_some()
    {
        let _result = runtime.set_expression(staging.expression, staging.weight);
    } else if runtime.asset().expressions().get("happy").is_some() {
        let _result = runtime.set_expression("happy", staging.weight * 0.4);
    }
    let _result = runtime.set_look_at_target(Some(glam::Vec3::from_array(staging.look_at)));
}

fn apply_clip_staging(runtime: &mut vrm_runtime::AvatarRuntime, pose: PoseHint) {
    runtime.clear_expressions();
    runtime.clear_bone_rotations();
    apply_expression(runtime, &staging(pose, 0.0));
}

fn apply_procedural_pose(
    runtime: &mut vrm_runtime::AvatarRuntime,
    pose: PoseHint,
    elapsed_secs: f32,
) {
    runtime.clear_expressions();
    runtime.clear_bone_rotations();
    let phase = elapsed_secs * std::f32::consts::TAU;
    let staging = staging(pose, phase);
    apply_expression(runtime, &staging);
    if runtime
        .asset()
        .humanoid()
        .bone(vrm_runtime::HumanBone::Head)
        .is_some()
    {
        let _result = runtime.set_bone_rotation(
            vrm_runtime::HumanBone::Head,
            glam::Quat::from_rotation_y(staging.head_yaw),
        );
    }
    if runtime
        .asset()
        .humanoid()
        .bone(vrm_runtime::HumanBone::Spine)
        .is_some()
    {
        let _result = runtime.set_bone_rotation(
            vrm_runtime::HumanBone::Spine,
            glam::Quat::from_rotation_z(staging.spine_roll * phase.sin()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::VrmSession;
    use crate::ipc::{
        AssetFailReason, AssetRef, FeatureSupport, MotionFailReason, MotionSetInfo, PoseClip,
        PoseHint,
    };
    use crate::testing::{MotionFixture, write_generated_vrm, write_generated_vrma};
    use std::io::Write as _;

    /// A rotating clip: the head turns 120 degrees over half a second.
    const HEAD_TURN: MotionFixture = MotionFixture {
        bone: "head",
        yaw_degrees: 120.0,
        duration_secs: 0.5,
    };

    /// A clip on a different bone, so which clip plays changes the frame and
    /// not only its timing.
    const SPINE_TURN: MotionFixture = MotionFixture {
        bone: "spine",
        yaw_degrees: -120.0,
        duration_secs: 0.5,
    };

    #[test]
    fn runtime_capabilities_are_adopted() {
        let session = VrmSession::new();
        assert_eq!(session.motion(), FeatureSupport::Unsupported);
        assert!(session.motion_poses().is_empty());
    }

    #[test]
    fn pose_order_lists_every_hint_once() {
        let order = super::POSE_ORDER;
        let mut unique = order.to_vec();
        unique.sort_by_key(|pose| super::pose_index(*pose));
        unique.dedup();
        assert_eq!(unique.len(), 5, "every hint must own exactly one slot");
        let mut slots = order
            .iter()
            .map(|pose| super::pose_index(*pose))
            .collect::<Vec<_>>();
        slots.sort_unstable();
        assert_eq!(slots, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn generated_vrm_1_fixture_loads_and_evaluates_every_pose() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("generated-runtime-probe.vrm");
        write_generated_vrm(&path).expect("fixture");
        let mut session = VrmSession::new();
        session
            .set_asset(AssetRef::Path {
                path: path.to_string_lossy().into_owned(),
            })
            .expect("strict VRM fixture load");
        let stats = session.stats().expect("stats");
        assert_eq!(stats.primitives, 1);
        assert!(stats.expressions >= 5);
        assert_eq!(stats.spring_chains, 1);
        for pose in [
            PoseHint::Idle,
            PoseHint::Listening,
            PoseHint::Speaking,
            PoseHint::Working,
            PoseHint::Attention,
        ] {
            session.set_pose(pose);
            let meshes = session.update(1.0 / 30.0).expect("runtime update");
            assert_eq!(meshes.len(), 1);
            assert_eq!(meshes[0].mesh.positions.len(), 3);
            assert_eq!(meshes[0].texcoords.len(), 3);
            let texture = meshes[0].texture.as_ref().expect("base-color texture");
            assert_eq!((texture.width, texture.height), (1, 1));
            assert_eq!(&*texture.rgba, &[80, 160, 240, 255]);
        }
    }

    #[test]
    fn invalid_asset_does_not_replace_current_runtime() {
        let dir = tempfile::tempdir().expect("tempdir");
        let good = dir.path().join("good.vrm");
        write_generated_vrm(&good).expect("good");
        let bad = dir.path().join("bad.vrm");
        std::fs::File::create(&bad)
            .expect("create")
            .write_all(b"not VRM")
            .expect("write");
        let mut session = VrmSession::new();
        session
            .set_asset(AssetRef::Path {
                path: good.to_string_lossy().into_owned(),
            })
            .expect("good load");
        let previous = session.stats();
        let error = session
            .set_asset(AssetRef::Path {
                path: bad.to_string_lossy().into_owned(),
            })
            .expect_err("bad load");
        assert_eq!(error.reason, AssetFailReason::InvalidVrm);
        assert_eq!(session.stats(), previous);
    }

    #[test]
    fn missing_asset_is_a_domain_failure() {
        let mut session = VrmSession::new();
        let error = session
            .set_asset(AssetRef::Path {
                path: "/no/such/ene-body-asset.vrm".into(),
            })
            .expect_err("missing");
        assert_eq!(error.reason, AssetFailReason::Missing);
        assert!(!error.detail.is_empty());
    }

    #[test]
    fn assigned_clip_replaces_the_hand_authored_pose() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut plain = loaded_session(dir.path(), "plain", &[]);
        let mut clipped = loaded_session(dir.path(), "clipped", &[(PoseHint::Idle, HEAD_TURN)]);
        assert_eq!(clipped.motion(), FeatureSupport::Available);
        assert_eq!(clipped.motion_poses(), vec![PoseHint::Idle]);
        assert_eq!(plain.motion(), FeatureSupport::Unsupported);
        assert_ne!(
            first_frame(&mut plain),
            first_frame(&mut clipped),
            "an assigned clip must drive the pose instead of the staged sway"
        );
    }

    #[test]
    fn expressions_stay_hand_authored_while_a_clip_plays() {
        let dir = tempfile::tempdir().expect("tempdir");
        // The same clip on two hints: the clip and the spring state are equal,
        // so a differing frame shows the expression staging is still applied.
        let mut idle = loaded_session(dir.path(), "idle", &[(PoseHint::Idle, HEAD_TURN)]);
        let mut attention =
            loaded_session(dir.path(), "attention", &[(PoseHint::Attention, HEAD_TURN)]);
        attention.set_pose(PoseHint::Attention);
        assert_ne!(first_frame(&mut idle), first_frame(&mut attention));
    }

    #[test]
    fn switching_hint_plays_that_hints_clip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut session = loaded_session(
            dir.path(),
            "both",
            &[
                (PoseHint::Idle, HEAD_TURN),
                (PoseHint::Speaking, SPINE_TURN),
            ],
        );
        assert_eq!(
            session.motion_poses(),
            vec![PoseHint::Idle, PoseHint::Speaking]
        );
        let idle_frame = first_frame(&mut session);
        session.set_pose(PoseHint::Speaking);
        let speaking_frame = first_frame(&mut session);
        assert_ne!(idle_frame, speaking_frame);
        // Hints without a clip keep producing frames next to clipped ones.
        session.set_pose(PoseHint::Listening);
        let listening_frame = first_frame(&mut session);
        assert!(
            listening_frame
                .iter()
                .flatten()
                .all(|value| value.is_finite())
        );
        assert_ne!(listening_frame, speaking_frame);
    }

    #[test]
    fn clip_playback_advances_across_updates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut session = loaded_session(dir.path(), "advance", &[(PoseHint::Idle, HEAD_TURN)]);
        let first = first_frame(&mut session);
        for _ in 0..8 {
            let _frame = session.update(1.0 / 30.0).expect("runtime update");
        }
        let later = first_frame(&mut session);
        assert_ne!(first, later, "a looping clip must keep moving");
    }

    #[test]
    fn replacing_the_asset_restarts_the_clip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let asset = dir.path().join("restart.vrm");
        write_generated_vrm(&asset).expect("vrm fixture");
        let clip = dir.path().join("restart.vrma");
        write_generated_vrma(&clip, HEAD_TURN).expect("clip");
        let motion_set = MotionSetInfo {
            clips: vec![PoseClip {
                pose: PoseHint::Idle,
                path: clip.to_string_lossy().into_owned(),
            }],
        };
        let reference = {
            let mut fresh = VrmSession::new();
            fresh
                .set_asset(AssetRef::Path {
                    path: asset.to_string_lossy().into_owned(),
                })
                .expect("asset");
            fresh.set_motions(&motion_set).expect("motions");
            first_frame(&mut fresh)
        };
        let mut session = VrmSession::new();
        session
            .set_asset(AssetRef::Path {
                path: asset.to_string_lossy().into_owned(),
            })
            .expect("asset");
        session.set_motions(&motion_set).expect("motions");
        for _ in 0..10 {
            let _frame = session.update(1.0 / 30.0).expect("runtime update");
        }
        session
            .set_asset(AssetRef::Path {
                path: asset.to_string_lossy().into_owned(),
            })
            .expect("asset replacement");
        assert_eq!(
            reference,
            first_frame(&mut session),
            "a replaced avatar must not inherit the previous clip time"
        );
    }

    #[test]
    fn a_rejected_motion_set_keeps_the_previous_assignment() {
        let dir = tempfile::tempdir().expect("tempdir");
        let good = dir.path().join("good.vrma");
        write_generated_vrma(&good, HEAD_TURN).expect("clip");
        let good_path = good.to_string_lossy().into_owned();
        let mut session = loaded_session(dir.path(), "kept", &[(PoseHint::Idle, HEAD_TURN)]);
        let previous = session.motion_poses();

        let missing = session
            .set_motions(&MotionSetInfo {
                clips: vec![PoseClip {
                    pose: PoseHint::Idle,
                    path: dir
                        .path()
                        .join("absent.vrma")
                        .to_string_lossy()
                        .into_owned(),
                }],
            })
            .expect_err("missing clip");
        assert_eq!(missing.reason, MotionFailReason::Missing);

        let corrupt = dir.path().join("corrupt.vrma");
        std::fs::File::create(&corrupt)
            .expect("create")
            .write_all(b"not a vrma")
            .expect("write");
        let invalid = session
            .set_motions(&MotionSetInfo {
                clips: vec![PoseClip {
                    pose: PoseHint::Idle,
                    path: corrupt.to_string_lossy().into_owned(),
                }],
            })
            .expect_err("corrupt clip");
        assert_eq!(invalid.reason, MotionFailReason::InvalidVrma);

        // A valid glTF that is not a VRMA document must fail the same way.
        let avatar = dir.path().join("avatar-not-clip.vrma");
        write_generated_vrm(&avatar).expect("vrm bytes");
        let wrong_profile = session
            .set_motions(&MotionSetInfo {
                clips: vec![PoseClip {
                    pose: PoseHint::Idle,
                    path: avatar.to_string_lossy().into_owned(),
                }],
            })
            .expect_err("VRM is not a clip");
        assert_eq!(wrong_profile.reason, MotionFailReason::InvalidVrma);

        let duplicated = session
            .set_motions(&MotionSetInfo {
                clips: vec![
                    PoseClip {
                        pose: PoseHint::Idle,
                        path: good_path.clone(),
                    },
                    PoseClip {
                        pose: PoseHint::Idle,
                        path: good_path,
                    },
                ],
            })
            .expect_err("duplicate pose");
        assert_eq!(duplicated.reason, MotionFailReason::DuplicatePose);

        assert_eq!(session.motion(), FeatureSupport::Available);
        assert_eq!(
            session.motion_poses(),
            previous,
            "a rejected set must not clear playback"
        );
    }

    /// A session holding the generated avatar plus the given pose → clip pairs.
    fn loaded_session(
        dir: &std::path::Path,
        name: &str,
        clips: &[(PoseHint, MotionFixture)],
    ) -> VrmSession {
        let asset = dir.join(std::format!("{name}.vrm"));
        write_generated_vrm(&asset).expect("vrm fixture");
        let mut session = VrmSession::new();
        session
            .set_asset(AssetRef::Path {
                path: asset.to_string_lossy().into_owned(),
            })
            .expect("asset load");
        if clips.is_empty() {
            return session;
        }
        let motion_set = MotionSetInfo {
            clips: clips
                .iter()
                .enumerate()
                .map(|(index, (pose, fixture))| {
                    let path = dir.join(std::format!("{name}-{index}.vrma"));
                    write_generated_vrma(&path, *fixture).expect("vrma fixture");
                    PoseClip {
                        pose: *pose,
                        path: path.to_string_lossy().into_owned(),
                    }
                })
                .collect(),
        };
        session.set_motions(&motion_set).expect("motion set");
        session
    }

    /// Advances one frame and returns the generated primitive's vertices.
    fn first_frame(session: &mut VrmSession) -> Vec<[f32; 3]> {
        let meshes = session.update(1.0 / 30.0).expect("runtime update");
        meshes
            .first()
            .map(|mesh| mesh.mesh.positions.clone())
            .expect("one generated primitive")
    }
}
