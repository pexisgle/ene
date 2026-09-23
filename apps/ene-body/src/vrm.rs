//! Body-local VRM 1.0 runtime.
//!
//! `vrm-runtime` is intentionally confined to this crate. The parent projects
//! only [`PoseHint`]; expression weights, LookAt targets,
//! humanoid rotations, SpringBone state, and renderer frame data never cross
//! IPC.
//!
//! An activity hint plays the `.vrma` clip the parent assigned to it. Hints
//! without a clip keep the hand-authored staging, so a missing motion pack
//! degrades to the previous behavior instead of faking playback.

use crate::ipc::{
    AssetFailInfo, AssetFailReason, AssetRef, FeatureSupport, MotionFailInfo, MotionFailReason,
    MotionSetInfo, PoseHint,
};
use std::sync::Arc;

use vrm_runtime::{PlaybackMode, PlaybackOptions, VrmAnimation};

/// Pose hints in a fixed order so replay and reporting stay deterministic.
const POSE_ORDER: [PoseHint; 5] = [
    PoseHint::Idle,
    PoseHint::Listening,
    PoseHint::Speaking,
    PoseHint::Working,
    PoseHint::Attention,
];

/// Assigned clips loop: a hint is a standing activity class, not a one-shot.
const MOTION_PLAYBACK: PlaybackOptions = PlaybackOptions {
    speed: 1.0,
    mode: PlaybackMode::Loop,
    scale_hips_translation: true,
};

/// Validated renderer-facing information about the loaded avatar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct AssetStats {
    pub primitives: usize,
    pub expressions: usize,
    pub spring_chains: usize,
}

/// One CPU-deformed primitive plus its expression-evaluated PBR base color.
/// Texture sampling and MToon remain renderer quality work; geometry,
/// skinning, morphs, vertex colors, and dynamic material color are retained.
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

/// Per-process VRM session. Holds one validated asset and its mutable runtime.
pub struct VrmSession {
    current: Option<AssetRef>,
    runtime: Option<vrm_runtime::AvatarRuntime>,
    motions: [Option<Arc<VrmAnimation>>; POSE_ORDER.len()],
    playing: Option<PoseHint>,
    pose: PoseHint,
    elapsed_secs: f32,
    stats: Option<AssetStats>,
    material_textures: Vec<Option<MaterialTexture>>,
    asset_generation: u32,
}

impl std::fmt::Debug for VrmSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VrmSession")
            .field("current", &self.current)
            .field("runtime_loaded", &self.runtime.is_some())
            .field("pose", &self.pose)
            .field("motion_poses", &self.motion_poses())
            .field("playing", &self.playing)
            .field("stats", &self.stats)
            .finish()
    }
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
            current: None,
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
    pub const fn expressions(&self) -> FeatureSupport {
        FeatureSupport::Available
    }

    #[must_use]
    pub const fn spring_bone(&self) -> FeatureSupport {
        FeatureSupport::Available
    }

    #[must_use]
    pub fn pose(&self) -> PoseHint {
        self.pose
    }

    #[must_use]
    pub fn current(&self) -> Option<&AssetRef> {
        self.current.as_ref()
    }

    #[must_use]
    pub fn stats(&self) -> Option<AssetStats> {
        self.stats
    }

    #[must_use]
    pub fn loaded(&self) -> bool {
        self.runtime.is_some()
    }

    /// Changes the Body-local activity projection. Runtime controls are
    /// applied on the next fixed-rate update.
    pub fn set_pose(&mut self, pose: PoseHint) {
        self.pose = pose;
    }

    /// Whether validated clips are loaded. Playback itself is per hint and is
    /// driven by [`Self::update`].
    #[must_use]
    pub fn motion(&self) -> FeatureSupport {
        if self.motions.iter().any(Option::is_some) {
            FeatureSupport::Available
        } else {
            FeatureSupport::Unsupported
        }
    }

    /// Hints that currently have a clip, in the fixed pose order replay uses.
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

    /// Validates and adopts a pose → clip assignment.
    ///
    /// Every clip is loaded and validated before any of them replaces playback
    /// state, so a set with one bad clip cannot partially disable motion. The
    /// previous assignment stays live on failure.
    ///
    /// # Errors
    ///
    /// Structural violations ([`MotionSetInfo::validate`]), unavailable or
    /// non-file paths, and `.vrma` files the runtime rejects.
    pub fn set_motions(&mut self, set: &MotionSetInfo) -> Result<(), MotionFailInfo> {
        set.validate()?;
        let mut loaded = Vec::with_capacity(set.clips.len());
        for clip in &set.clips {
            let path = clip.path.as_str();
            let meta = std::fs::metadata(path).map_err(|_| {
                MotionFailInfo::new(MotionFailReason::Missing, "motion clip file is unavailable")
            })?;
            if !meta.is_file() {
                return Err(MotionFailInfo::new(
                    MotionFailReason::NotAFile,
                    "motion clip path is not a regular file",
                ));
            }
            let animation = VrmAnimation::load(path).map_err(|error| {
                MotionFailInfo::new(
                    MotionFailReason::InvalidVrma,
                    std::format!("VRMA validation failed: {error}"),
                )
            })?;
            animation.validate().map_err(|error| {
                MotionFailInfo::new(
                    MotionFailReason::InvalidVrma,
                    std::format!("VRMA clip is not playable: {error}"),
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

    /// Strictly loads and validates a VRM 1.0 asset before replacing the
    /// current runtime. A failed replacement leaves the previous avatar live.
    ///
    /// # Errors
    ///
    /// Missing/non-file paths, invalid VRM 1.0, or an asset that cannot meet
    /// Stage 7's renderer/expression/LookAt/SpringBone contract.
    pub fn set_asset(&mut self, asset: AssetRef) -> Result<(), AssetFailInfo> {
        let path = asset.path();
        if path.is_empty() {
            return Err(fail(AssetFailReason::EmptyPath, "asset path is empty"));
        }
        let meta = std::fs::metadata(path)
            .map_err(|_| fail(AssetFailReason::Missing, "asset file is unavailable"))?;
        if !meta.is_file() {
            return Err(fail(
                AssetFailReason::NotAFile,
                "asset path is not a regular file",
            ));
        }
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
        let stats = AssetStats {
            primitives: avatar.primitives().len(),
            expressions: avatar.expressions().len(),
            spring_chains: avatar.spring_bone().springs.len(),
        };
        let asset_generation = self.asset_generation.saturating_add(1);
        let material_textures = decode_material_textures(&avatar, asset_generation)?;
        let mut runtime = vrm_runtime::AvatarRuntime::new(avatar);
        let frame = runtime.update(0.0).map_err(|error| {
            fail(
                AssetFailReason::RuntimeEvaluation,
                format!("initial runtime evaluation failed: {error}"),
            )
        })?;
        if frame.gpu().draws().len() == 0 {
            return Err(fail(
                AssetFailReason::MissingPrimitives,
                "runtime produced no renderer draws",
            ));
        }
        self.current = Some(asset);
        self.runtime = Some(runtime);
        self.playing = None;
        self.elapsed_secs = 0.0;
        self.stats = Some(stats);
        self.material_textures = material_textures;
        self.asset_generation = asset_generation;
        Ok(())
    }

    /// Evaluates expression, LookAt, humanoid motion, and SpringBone and
    /// returns CPU-deformed meshes for the simple wgpu fallback renderer.
    ///
    /// # Errors
    ///
    /// Runtime evaluation failure. No asset yields an empty frame.
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
                // The clip is started once per hint change: restarting every
                // frame would pin playback to time zero.
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
    let mut decoded = vec![None; avatar.textures().len()];
    for (id, texture) in avatar.textures().iter().enumerate() {
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

fn transformed_uv(uv: [f32; 2], transform: vrm_runtime::TextureTransform) -> [f32; 2] {
    let scaled = [uv[0] * transform.scale[0], uv[1] * transform.scale[1]];
    let (sin, cos) = transform.rotation.sin_cos();
    [
        transform.offset[0] + cos * scaled[0] - sin * scaled[1],
        transform.offset[1] + sin * scaled[0] + cos * scaled[1],
    ]
}

fn animated_uv(uv: [f32; 2], animation: vrm_runtime::UvAnimationState) -> [f32; 2] {
    let (sin, cos) = animation.rotation.sin_cos();
    [
        animation.scroll[0] + cos * uv[0] - sin * uv[1],
        animation.scroll[1] + sin * uv[0] + cos * uv[1],
    ]
}

fn fail(reason: AssetFailReason, detail: impl Into<String>) -> AssetFailInfo {
    AssetFailInfo {
        reason,
        detail: detail.into(),
    }
}

/// Slot of a pose hint inside the fixed pose order.
fn pose_index(pose: PoseHint) -> usize {
    match pose {
        PoseHint::Idle => 0,
        PoseHint::Listening => 1,
        PoseHint::Speaking => 2,
        PoseHint::Working => 3,
        PoseHint::Attention => 4,
    }
}

/// Hand-authored expression, gaze, and sway for one activity hint.
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

/// Clip playback owns every humanoid rotation, so only the expression and gaze
/// of the activity staging are layered on. Applying the hand-authored head and
/// spine rotations on top would compose two claims about the same bones.
fn apply_clip_staging(runtime: &mut vrm_runtime::AvatarRuntime, pose: PoseHint) {
    runtime.clear_expressions();
    runtime.clear_bone_rotations();
    apply_expression(runtime, &staging(pose, 0.0));
}

/// Staging for activity hints that have no clip assigned.
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
