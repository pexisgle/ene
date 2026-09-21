//! Body-local VRM 1.0 runtime.
//!
//! `vrm-runtime` is intentionally confined to this crate. The parent projects
//! only [`PoseHint`]; expression weights, LookAt targets,
//! humanoid rotations, SpringBone state, and renderer frame data never cross
//! IPC.

use crate::ipc::{AssetFailInfo, AssetFailReason, AssetRef, FeatureSupport, PoseHint};
use std::sync::Arc;

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
        let Some(runtime) = self.runtime.as_mut() else {
            return Ok(Vec::new());
        };
        self.elapsed_secs = (self.elapsed_secs + dt).rem_euclid(120.0);
        apply_pose(runtime, self.pose, self.elapsed_secs);
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

fn apply_pose(runtime: &mut vrm_runtime::AvatarRuntime, pose: PoseHint, elapsed_secs: f32) {
    runtime.clear_expressions();
    runtime.clear_bone_rotations();
    let phase = elapsed_secs * std::f32::consts::TAU;
    let (expression, weight, look_target, head_yaw, spine_roll) = match pose {
        PoseHint::Idle => ("relaxed", 0.25, [0.0, 1.45, 2.0], phase.sin() * 0.025, 0.01),
        PoseHint::Listening => ("relaxed", 0.45, [0.2, 1.5, 1.8], -0.08, -0.02),
        PoseHint::Speaking => ("aa", 0.75, [0.0, 1.5, 2.0], phase.sin() * 0.06, 0.025),
        PoseHint::Working => ("neutral", 0.6, [0.0, 1.0, 1.4], 0.03, -0.035),
        PoseHint::Attention => ("surprised", 0.65, [0.0, 1.6, 1.5], 0.0, 0.0),
    };
    if runtime.asset().expressions().get(expression).is_some() {
        let _result = runtime.set_expression(expression, weight);
    } else if runtime.asset().expressions().get("happy").is_some() {
        let _result = runtime.set_expression("happy", weight * 0.4);
    }
    let _result = runtime.set_look_at_target(Some(glam::Vec3::from_array(look_target)));
    if runtime
        .asset()
        .humanoid()
        .bone(vrm_runtime::HumanBone::Head)
        .is_some()
    {
        let _result = runtime.set_bone_rotation(
            vrm_runtime::HumanBone::Head,
            glam::Quat::from_rotation_y(head_yaw),
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
            glam::Quat::from_rotation_z(spine_roll * phase.sin()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::VrmSession;
    use crate::ipc::{AssetFailReason, AssetRef, FeatureSupport, PoseHint};
    use serde_json::{Value, json};
    use std::io::Write as _;

    const BONES: [&str; 15] = [
        "hips",
        "spine",
        "head",
        "leftUpperLeg",
        "leftLowerLeg",
        "leftFoot",
        "rightUpperLeg",
        "rightLowerLeg",
        "rightFoot",
        "leftUpperArm",
        "leftLowerArm",
        "leftHand",
        "rightUpperArm",
        "rightLowerArm",
        "rightHand",
    ];

    #[test]
    fn runtime_capabilities_are_adopted() {
        let session = VrmSession::new();
        assert_eq!(session.expressions(), FeatureSupport::Available);
        assert_eq!(session.spring_bone(), FeatureSupport::Available);
        assert_eq!(session.pose(), PoseHint::Idle);
    }

    #[test]
    fn generated_vrm_1_fixture_loads_and_evaluates_every_pose() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("generated-runtime-probe.vrm");
        std::fs::write(&path, fixture_glb()).expect("fixture");
        let mut session = VrmSession::new();
        session
            .set_asset(AssetRef::BytesTemp {
                path: path.to_string_lossy().into_owned(),
            })
            .expect("strict VRM fixture load");
        assert!(session.loaded());
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
        std::fs::write(&good, fixture_glb()).expect("good");
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
        let previous = session.current().cloned();
        let error = session
            .set_asset(AssetRef::Path {
                path: bad.to_string_lossy().into_owned(),
            })
            .expect_err("bad load");
        assert_eq!(error.reason, AssetFailReason::InvalidVrm);
        assert_eq!(session.current(), previous.as_ref());
        assert!(session.loaded());
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

    fn fixture_glb() -> Vec<u8> {
        let mut nodes = BONES
            .iter()
            .enumerate()
            .map(|(index, bone)| {
                json!({
                    "name": bone,
                    "translation": [0.0, index as f32 * 0.05, 0.0]
                })
            })
            .collect::<Vec<_>>();
        nodes[0]["children"] = json!([1, 3, 6]);
        nodes[1]["children"] = json!([2, 9, 12]);
        nodes[2]["mesh"] = json!(0);
        nodes[3]["children"] = json!([4]);
        nodes[4]["children"] = json!([5]);
        nodes[6]["children"] = json!([7]);
        nodes[7]["children"] = json!([8]);
        nodes[9]["children"] = json!([10]);
        nodes[10]["children"] = json!([11]);
        nodes[12]["children"] = json!([13]);
        nodes[13]["children"] = json!([14]);
        let human_bones = BONES
            .iter()
            .enumerate()
            .map(|(index, bone)| (bone.to_string(), json!({ "node": index })))
            .collect::<serde_json::Map<_, _>>();
        let expression = || {
            json!({
                "morphTargetBinds": [{ "node": 2, "index": 0, "weight": 1.0 }]
            })
        };
        let mut encoded_png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(1, 1, vec![80, 160, 240, 255]).expect("fixture image"),
        )
        .write_to(&mut encoded_png, image::ImageFormat::Png)
        .expect("encode fixture PNG");
        let encoded_png = encoded_png.into_inner();
        let mut document = json!({
            "asset": { "version": "2.0" },
            "extensionsUsed": ["VRMC_vrm", "VRMC_springBone"],
            "extensions": {
                "VRMC_vrm": {
                    "specVersion": "1.0",
                    "meta": {
                        "name": "Generated ene-body runtime fixture",
                        "authors": ["ene tests"],
                        "licenseUrl": "https://creativecommons.org/publicdomain/zero/1.0/"
                    },
                    "humanoid": { "humanBones": human_bones },
                    "lookAt": { "type": "bone", "offsetFromHeadBone": [0.0, 0.1, 0.0] },
                    "expressions": { "preset": {
                        "happy": expression(),
                        "relaxed": expression(),
                        "aa": expression(),
                        "surprised": expression(),
                        "neutral": expression()
                    }}
                },
                "VRMC_springBone": {
                    "specVersion": "1.0",
                    "springs": [{
                        "name": "head fixture spring",
                        "joints": [{ "node": 2, "stiffness": 1.0, "dragForce": 0.4 }]
                    }]
                }
            },
            "nodes": nodes,
            "scenes": [{ "nodes": [0] }],
            "scene": 0,
            "buffers": [{ "byteLength": 96 + encoded_png.len() }],
            "bufferViews": [
                { "buffer": 0, "byteOffset": 0, "byteLength": 36 },
                { "buffer": 0, "byteOffset": 36, "byteLength": 36 },
                { "buffer": 0, "byteOffset": 72, "byteLength": 24 },
                { "buffer": 0, "byteOffset": 96, "byteLength": encoded_png.len() }
            ],
            "accessors": [
                {
                    "bufferView": 0,
                    "componentType": 5126,
                    "count": 3,
                    "type": "VEC3",
                    "min": [-0.5, 0.0, 0.0],
                    "max": [0.5, 1.0, 0.0]
                },
                { "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" },
                { "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2" }
            ],
            "images": [{ "bufferView": 3, "mimeType": "image/png" }],
            "textures": [{ "source": 0 }],
            "materials": [{ "pbrMetallicRoughness": {
                "baseColorFactor": [0.5, 0.75, 1.0, 1.0],
                "baseColorTexture": { "index": 0 }
            }}],
            "meshes": [{ "primitives": [{
                "attributes": { "POSITION": 0, "TEXCOORD_0": 2 },
                "material": 0,
                "targets": [{ "POSITION": 1 }]
            }] }]
        });
        document["nodes"][2]["mesh"] = json!(0);
        let positions_and_target = [
            -0.5_f32, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 1.0, 0.0, // triangle
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.05, 0.0, 0.0, // expression delta
        ];
        let binary = positions_and_target
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .chain(
                [0.0_f32, 0.0, 1.0, 0.0, 0.5, 1.0]
                    .iter()
                    .flat_map(|value| value.to_le_bytes()),
            )
            .chain(encoded_png)
            .collect::<Vec<_>>();
        glb(&document, &binary)
    }

    fn glb(document: &Value, binary: &[u8]) -> Vec<u8> {
        let mut json = serde_json::to_vec(document).expect("serialize fixture");
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let mut bin = binary.to_vec();
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let total = 12 + 8 + json.len() + 8 + bin.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2_u32.to_le_bytes());
        out.extend_from_slice(&u32::try_from(total).expect("fixture size").to_le_bytes());
        out.extend_from_slice(&u32::try_from(json.len()).expect("JSON size").to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json);
        out.extend_from_slice(&u32::try_from(bin.len()).expect("binary size").to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
        out
    }
}
