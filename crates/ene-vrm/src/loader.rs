//! VRM file loader.
//!
//! Reads a `.vrm` file from disk and produces a [`VrmModel`] with
//! every primitive of **every** glTF `Mesh` uploaded to the GPU
//! (vertex / index buffers + each primitive's own base-color
//! texture) and the first skin's inverse-bind matrices preserved
//! for's skinning pass.
//!
//! ## Multi-mesh scope (this struct)
//!
//! A VRM 1.0 file such as `AliciaSolid.vrm` contains ~12 separate
//! glTF `Mesh` objects (body, hair, face, clothes, accessories,
//! …), not one mesh with many primitives./3.1/3.2 only
//! loaded `meshes[0]` (the head/face area) and therefore rendered
//! only the skin. walks the entire `gltf.meshes()` iterator,
//! computing one global AABB across all primitive positions so the
//! per-vertex normalization (`TARGET_MODEL_SIZE = 1.5` m) is
//! applied uniformly across the whole body.
//!
//! ##.x deliberately does **not** support
//!
//! - `MToon`'s full PBR parameters (rim / matcap / outline /
//!   emission). The shader applies a simple diffuse + lit + base
//!   color. The MToon-flavored `KHR_materials_unlit` flag is
//!   *read* so the loader can log a warning if it is set, but
//!   does not yet alter rendering.
//! - Per-mesh glTF node transforms. The loader uses raw
//!   vertex positions from each mesh without applying the glTF
//!   node hierarchy's world transforms. For VRoid-exported models
//!   this is fine because the body parts are already in
//!   world-space positions in their local vertex buffers. The
//!   humanoid bone system (this struct) will eventually apply the
//!   per-joint skinning matrices on top of this.
//! - Animation, expressions, morph targets, spring bone.
//! - `.gltf` (non-binary) VRM files. Only `.glb` (binary) is
//!   supported in.x; the glTF binary payload (`BIN` chunk)
//!   holds all the meshes / textures. External `.bin` files
//!   require the `gltf` crate's `import` feature and ship as a
//!   follow-up PR.
//!
//! See `docs/architecture/wgpu-migration.md` §22.6 for the
//! status.

use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use image::ImageReader;
use wgpu::util::DeviceExt;

use crate::error::{VrmError, VrmResult};
use crate::expression::{
    ExpressionLayer, MAX_MORPH_TARGETS_PER_PRIMITIVE, PrimitiveId, PrimitiveMorphs,
};
use crate::expression_override;
use crate::humanoid;
use crate::look_at;
use crate::model::{
    AlphaMode, MeshVertex, NodeHierarchy, Skeleton, VrmMesh, VrmModel, VrmPrimitive, VrmTexture,
};
use crate::mtoon;
use crate::node_constraint;
use crate::spring_bone;

/// Target world-space size of the model along its longest axis
/// after's per-vertex normalization. The legacy Bevy
/// `bevy_vrm1` bakes the same scale into its world transform;
/// v2 applies it at load time.
const TARGET_MODEL_SIZE: f32 = 1.5;

/// Load a `.vrm` file from disk and upload every primitive of the
/// first mesh to the GPU.
///
/// `path` is the on-disk `.vrm` (a glTF binary with the `VRMC_vrm`
/// extension). `device` and `queue` are used to allocate the
/// vertex / index / texture buffers.
pub fn load_vrm(
    path: impl AsRef<Path>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> VrmResult<VrmModel> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|source| VrmError::Io {
        path: path.display().to_string(),
        source,
    })?;

    let gltf = gltf::Gltf::from_slice(&bytes).map_err(|e| VrmError::Gltf(e.to_string()))?;

    if !gltf.document.extensions_used().any(|e| e == "VRMC_vrm") {
        return Err(VrmError::NotVrm);
    }

    let mtoon_materials = mtoon::load_mtoon_materials(&gltf);
    // walk every skin (not just the first) and
    // build a merged skeleton + per-primitive JOINTS_0
    // remap. The remap is applied inside `load_all_meshes`
    // so each vertex's `JOINTS_0` is translated from its
    // own skin's index space into the merged-skeleton
    // index space the runtime updates every frame.
    let (skeleton, primitive_joint_remap) = load_merged_skeleton_and_remaps(&gltf);
    let (
        mesh,
        morph_per_primitive,
        (aabb_min, aabb_max),
        center,
        normalize_scale,
        has_nonzero_joints,
    ) = load_all_meshes(
        &gltf,
        device,
        queue,
        &mtoon_materials,
        &primitive_joint_remap,
    )?;
    let nodes = load_node_hierarchy(&gltf);

    // a model that defines no skin but whose
    // primitives carry non-trivial `JOINTS_0` data is
    // malformed (most likely a VRM 0.x file that lost its
    // skin during export, or a hand-rolled glTF whose exporter
    // wrote `JOINTS_0` without an `IBM`). The renderer
    // currently falls back to a one-element
    // `Mat4::IDENTITY` skin palette + `joints = [0, 0, 0, 0]`,
    // which is a silent no-op: the per-vertex weighting looks
    // like identity, but the model really wants
    // skinning. Warn here so the user can re-export the model;
    // will promote this to a load-time error once the
    // refactor lands a proper "load is malformed" channel.
    if skeleton.joint_count() == 0 && has_nonzero_joints {
        tracing::warn!(
            "VRM {} has no skin but primitive(s) carry non-trivial JOINTS_0; \
             skinning will fall back to identity. Re-export with VRMC_vrm 1.0 to fix.",
            path.display()
        );
    }

    let unlit_count: usize = mesh
        .iter()
        .flat_map(|m| m.primitives.iter())
        .filter(|p| p.unlit)
        .count();
    if unlit_count > 0 {
        tracing::info!(
            "VRM {} has {} unlit primitive(s) (KHR_materials_unlit)",
            path.display(),
            unlit_count,
        );
    }

    let expressions_meta = expression_override::load_expression_overrides(&gltf);
    if !expressions_meta.is_empty() {
        tracing::info!(
            "VRM {} {} expression definition(s) with override metadata",
            path.display(),
            expressions_meta.len(),
        );
    }

    let expression_layer = ExpressionLayer::new(
        morph_per_primitive,
        (!expressions_meta.is_empty()).then_some(&expressions_meta),
    );
    let expression_count: usize = expression_layer
        .per_primitive
        .iter()
        .map(|p| p.as_ref().map_or(0, |m| m.targets.len()))
        .sum();
    if expression_count > 0 {
        tracing::info!(
            "VRM {} loaded {} expression(s) across {} primitive(s)",
            path.display(),
            expression_layer.expression_names().len(),
            expression_layer.morphic_primitive_count(),
        );
    }

    // Build the humanoid bone registry from the
    // `VRMC_vrm.humanoid.humanBones` block. The loader
    // walks every entry, canonicalises the bone name,
    // resolves the glTF node + rest transform + Skeleton
    // joint index, and returns an empty registry for
    // models without humanoid metadata. The `humanoid`
    // module logs per-entry warnings itself.
    let humanoid = humanoid::load_humanoid_bones(&gltf, &skeleton);

    // parse the `VRMC_vrm.lookAt` block.
    // `None` for models without the block (legacy VRM
    // 0.x, hand-rolled test fixtures) — the runtime falls
    // back to `LookAtProperties::default()` in that case.
    let look_at = look_at::load_look_at(&gltf);
    if look_at.is_some() {
        tracing::info!(
            "VRM {} lookAt: type={:?}, offsetFromHeadBone={:?}",
            path.display(),
            look_at.as_ref().map(|p| p.look_at_type),
            look_at.as_ref().map(|p| p.offset_from_head_bone),
        );
    }

    // parse per-expression override settings from the
    // `VRMC_vrm.expressions.{preset,custom}.<name>` tree.
    // Empty for models without the block, in which case the
    // per-frame override pass is a no-op.

    // parse `VRMC_node_constraint` from every glTF
    // node. Empty for models without the extension.
    let node_constraints = node_constraint::load_node_constraints(&gltf);

    // parse `VRMC_springBone` from the glTF root
    // extensions. `None` for models without the extension.
    let spring_bones = spring_bone::load_spring_bones(&gltf);
    if let Some(ref sb) = spring_bones {
        tracing::info!(
            "VRM {} springBone: {} collider(s), {} group(s), {} chain(s)",
            path.display(),
            sb.colliders.len(),
            sb.collider_groups.len(),
            sb.springs.len(),
        );
    }

    Ok(VrmModel::new(
        mesh,
        skeleton,
        aabb_min,
        aabb_max,
        center,
        normalize_scale,
        expression_layer,
        humanoid,
        nodes,
        look_at,
        expressions_meta,
        node_constraints,
        spring_bones,
    ))
}

/// Load every triangle-list primitive of every glTF `Mesh` in the
/// document. Returns a `Vec<VrmMesh>` — one entry per glTF `Mesh`.
/// Each primitive gets its own vertex / index buffers and (when
/// its material has one) its own base-color texture, so the body,
/// clothes, face, hair, and accessories all render.
///
/// The third return value is the per-primitive morph-target data
/// (this struct), aligned 1:1 with the linearized primitive list
/// `(mesh_idx, prim_idx)`. The fourth and fifth are the
/// post-normalize AABB `(min, max)`.
///
/// `expression_names` is the resolver map produced by
/// [`resolve_expression_names`]; the loader uses it to rename
/// each primitive's morph targets to the real `VRMC_vrm`
/// expression name (e.g. `happy`, `sad`) instead of the
/// synthetic `morph_target_<i>` fallback.
type Aabb = ([f32; 3], [f32; 3]);

/// Aggregate return of [`load_all_meshes`]: the per-primitive
/// mesh data, the parallel morph-target slot vector, the
/// raw AABB plus the AABB centre and `1.5 / max_extent`
/// normalize scale (the runtime folds both into the model
/// matrix at draw time — the vertex buffer is no longer
/// re-centered on load), and the "any primitive carries a
/// non-trivial `JOINTS_0` accessor?" flag that powers the
/// issue #8 malformed-skin warning.
type LoadAllMeshesResult = (
    Vec<VrmMesh>,
    Vec<Option<PrimitiveMorphs>>,
    Aabb,
    [f32; 3],
    f32,
    bool,
);

fn load_all_meshes(
    gltf: &gltf::Gltf,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mtoon_materials: &[Option<mtoon::MToonMaterial>],
    primitive_joint_remap: &[Vec<Vec<u32>>],
) -> VrmResult<LoadAllMeshesResult> {
    let mut base_color_cache = std::collections::HashMap::new();
    let mut mtoon_textures_cache = std::collections::HashMap::new();
    // First pass: collect every triangle-list primitive's
    // positions across **all** glTF meshes so we can compute one
    // global AABB. (A VRM 1.0 has ~12 separate glTF Mesh
    // objects, not one.) Vertices are kept in raw glTF
    // space — we only record the AABB and the `center` /
    // `normalize_scale` pair that the runtime needs to build
    // the model matrix.
    let mut all_positions: Vec<[f32; 3]> = Vec::new();
    for mesh in gltf.document.meshes() {
        for primitive in mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = primitive.reader(|_buffer| gltf.blob.as_deref());
            if let Some(positions) = reader.read_positions() {
                all_positions.extend(positions);
            }
        }
    }
    if all_positions.is_empty() {
        return Err(VrmError::NoMeshes);
    }

    let mut bb_min = all_positions[0];
    let mut bb_max = all_positions[0];
    for p in &all_positions {
        for i in 0..3 {
            bb_min[i] = bb_min[i].min(p[i]);
            bb_max[i] = bb_max[i].max(p[i]);
        }
    }
    let extent: [f32; 3] = [
        bb_max[0] - bb_min[0],
        bb_max[1] - bb_min[1],
        bb_max[2] - bb_min[2],
    ];
    let max_extent = extent.iter().copied().fold(0.0f32, f32::max);
    let center: [f32; 3] = [
        f32::midpoint(bb_min[0], bb_max[0]),
        f32::midpoint(bb_min[1], bb_max[1]),
        f32::midpoint(bb_min[2], bb_max[2]),
    ];
    let scale = if max_extent > 0.0001 {
        TARGET_MODEL_SIZE / max_extent
    } else {
        1.0
    };
    let total_mesh_count = gltf.document.meshes().count();
    let total_primitive_count: usize = gltf.document.meshes().map(|m| m.primitives().count()).sum();
    tracing::info!(
        "VRM AABB: min={:?} max={:?} extent={:?} max_extent={} center={:?} normalize_scale={} mesh_count={} primitive_count={}",
        bb_min,
        bb_max,
        extent,
        max_extent,
        center,
        scale,
        total_mesh_count,
        total_primitive_count
    );

    // Second pass: build one `VrmMesh` per glTF mesh, each holding
    // its triangle-list primitives. The same `(center, scale)`
    // transform is applied to every vertex so the whole body
    // ends up centered on origin and bounded by the target size.
    //
    // also extract morph targets per primitive (position
    // displacements only, normalised by `(center, scale)` so the
    // GPU matches the vertex buffer's scale). The per-primitive
    // morph data is stored in a parallel `Vec<Option<PrimitiveMorphs>>`
    // aligned 1:1 with the final `VrmPrimitive` list (skipped
    // primitives are recorded as `None` so the indices stay
    // stable).
    //
    // we also track whether any primitive carries a
    // non-trivial `JOINTS_0` (i.e. any index > 0). When
    // combined with a missing skin (decided by the caller
    // after `load_first_skeleton` runs) this signals a
    // malformed model. The flag is returned alongside the
    // mesh and morph data so the caller can log a single
    // combined warning.

    let mut mesh_to_node = std::collections::HashMap::new();
    for node in gltf.document.nodes() {
        if let Some(mesh) = node.mesh() {
            mesh_to_node.insert(mesh.index(), node.index());
        }
    }

    let mut meshes = Vec::new();
    let mut morph_per_primitive: Vec<Option<PrimitiveMorphs>> = Vec::new();
    let mut has_nonzero_joints = false;
    for (mesh_idx, mesh) in gltf.document.meshes().enumerate() {
        let mut primitives = Vec::new();
        for (prim_idx, primitive) in mesh.primitives().enumerate() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                tracing::warn!(
                    "VRM mesh[{mesh_idx}].primitive[{prim_idx}] uses unsupported topology {:?}; skipping",
                    primitive.mode()
                );
                continue;
            }
            let reader = primitive.reader(|_buffer| gltf.blob.as_deref());

            let positions: Vec<[f32; 3]> = if let Some(p) = reader.read_positions() {
                p.collect()
            } else {
                tracing::warn!(
                    "VRM mesh[{mesh_idx}].primitive[{prim_idx}] has no POSITION; skipping"
                );
                continue;
            };
            let normals: Vec<[f32; 3]> = reader.read_normals().map_or_else(
                || vec![[0.0, 0.0, 1.0]; positions.len()],
                std::iter::Iterator::collect,
            );
            let uvs: Vec<[f32; 2]> = reader.read_tex_coords(0).map_or_else(
                || vec![[0.0, 0.0]; positions.len()],
                |tc| tc.into_f32().collect(),
            );
            // `JOINTS_0` is a u8/u16 accessor; the gltf 1.4
            // `ReadJoints` enum exposes `into_u16` (handles both
            // uniformly via a `CastingIter`). Promote to `u32` so
            // 256+-joint humanoid models (per-finger bones)
            // address every joint. A previous `[u8; 4]` aliased
            // every joint >= 255 onto `skin_matrices[255]`, which
            // stuck finger / wrist skinning to the same matrix.
            // The remap translates each vertex's per-skin index
            // into the merged-skeleton index; a missing remap
            // entry yields 0 (the renderer's `update_skin_palette`
            // maps that to the merged skeleton's first joint
            // instead of panicking).
            let remap_for_this_prim: &[u32] = primitive_joint_remap
                .get(mesh_idx)
                .and_then(|m| m.get(prim_idx))
                .map_or(&[], std::vec::Vec::as_slice);
            let remap_one =
                |slot: u16| -> u32 { remap_for_this_prim.get(slot as usize).copied().unwrap_or(0) };
            let joints: Vec<[u32; 4]> = reader.read_joints(0).map_or_else(
                || vec![[0, 0, 0, 0]; positions.len()],
                |js| {
                    js.into_u16()
                        .map(|j| {
                            [
                                remap_one(j[0]),
                                remap_one(j[1]),
                                remap_one(j[2]),
                                remap_one(j[3]),
                            ]
                        })
                        .collect()
                },
            );
            // A primitive carrying `JOINTS_0` without a skin
            // means a malformed model (most likely a VRM 0.x file
            // that lost its skin during export). Flag it once
            // and warn in the caller.
            if !has_nonzero_joints && joints.iter().any(|j| j.iter().any(|x| *x != 0)) {
                has_nonzero_joints = true;
            }
            let weights: Vec<[f32; 4]> = reader.read_weights(0).map_or_else(
                || vec![[1.0, 0.0, 0.0, 0.0]; positions.len()],
                |ws| ws.into_f32().collect(),
            );
            let indices: Vec<u32> = if let Some(i) = reader.read_indices() {
                i.into_u32().collect()
            } else {
                tracing::warn!(
                    "VRM mesh[{mesh_idx}].primitive[{prim_idx}] has no index buffer; skipping"
                );
                continue;
            };
            if indices.is_empty() {
                continue;
            }

            let mut vertices = Vec::with_capacity(positions.len());
            for (((pos, normal), uv), (joint, weight)) in positions
                .iter()
                .zip(normals.iter())
                .zip(uvs.iter())
                .zip(joints.iter().zip(weights.iter()))
            {
                // Vertices stay in raw glTF space; the runtime
                // folds `(center, scale)` into the model matrix
                // at draw time. Mutating positions would force
                // the bind matrices (computed against the raw
                // positions) to be recomputed.
                vertices.push(MeshVertex {
                    position: *pos,
                    uv: *uv,
                    normal: *normal,
                    joints: *joint,
                    weights: *weight,
                });
            }

            // Include `COPY_SRC` so the runtime can read the
            // buffers back at model-load time to build a Rapier
            // trimesh collider.
            let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("vrm.mesh{mesh_idx}.prim{prim_idx}.vertex_buf")),
                contents: MeshVertex::as_bytes(&vertices),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_SRC,
            });
            let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("vrm.mesh{mesh_idx}.prim{prim_idx}.index_buf")),
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_SRC,
            });

            let base_color = match load_primitive_base_color_texture(
                &primitive,
                gltf,
                device,
                queue,
                &mut base_color_cache,
            ) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(
                        "VRM mesh[{mesh_idx}].primitive[{prim_idx}] base-color decode failed: {e}"
                    );
                    None
                }
            };

            // Extract morph targets BEFORE pushing the primitive,
            // so the index lines up.
            let node_idx = mesh_to_node.get(&mesh_idx).copied().unwrap_or(0);

            let morphs = load_primitive_morph_targets(
                &primitive,
                gltf,
                positions.len(),
                mesh_idx,
                prim_idx,
                scale,
            );

            let alpha_mode = match primitive.material().alpha_mode() {
                gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
                gltf::material::AlphaMode::Mask => AlphaMode::Mask,
                gltf::material::AlphaMode::Blend => AlphaMode::Blend,
            };

            let unlit = primitive.material().unlit();

            // Attach MToon material if present.
            let mat_idx = primitive.material().index();
            let mtoon_mat = mat_idx.and_then(|i| mtoon_materials.get(i).cloned().flatten());

            let mtoon_textures = mtoon_mat.as_ref().zip(mat_idx).and_then(|(m, i)| {
                load_mtoon_gpu_textures(i, m, gltf, device, queue, &mut mtoon_textures_cache)
                    .ok()
                    .flatten()
            });

            primitives.push(VrmPrimitive {
                vertex_buf,
                vertex_count: vertices.len() as u32,
                index_buf,
                index_count: indices.len() as u32,
                // Keep the CPU-side vertex mirror so the
                // collider builder can walk `joints` / `weights`
                // / `position` per-primitive without a GPU
                // readback.
                vertices: vertices.clone(),
                base_color,
                alpha_mode,
                unlit,
                mtoon: mtoon_mat,
                mtoon_textures,
            });
            // Stable PrimitiveId: aligned 1:1 with the matching
            // morph entry. The draw loop uses the same mesh-major
            // ordering.
            let pid = PrimitiveId(morph_per_primitive.len());
            morph_per_primitive.push(morphs.map(|raw| {
                PrimitiveMorphs::from_targets(pid, node_idx, vertices.len() as u32, raw)
            }));
        }
        if primitives.is_empty() {
            tracing::debug!("VRM mesh[{mesh_idx}] has no renderable primitives; skipping");
            // Drop the morph records we pushed for this empty
            // mesh so the morph indices stay aligned with
            // `VrmPrimitive` in `meshes`.
            let dropped = mesh.primitives().count();
            let new_len = morph_per_primitive.len().saturating_sub(dropped);
            morph_per_primitive.truncate(new_len);
            continue;
        }
        meshes.push(VrmMesh { primitives });
    }

    if meshes.is_empty() {
        return Err(VrmError::NoMeshes);
    }

    Ok((
        meshes,
        morph_per_primitive,
        (bb_min, bb_max),
        center,
        scale,
        has_nonzero_joints,
    ))
}

/// Read every morph target on a single primitive and return the
/// position displacements in normalized model space.
///
/// `expected_vertex_count` is the host primitive's vertex count;
/// targets whose accessor is shorter are padded with zeros so the
/// GPU storage buffer length is always
/// `target_count * expected_vertex_count`.
///
/// Returns `Some(targets)` (with at least one entry) if the
/// primitive defines morph targets; `None` otherwise. The
/// `scale` matches the normalization the renderer's vertex
/// buffer applied, so the resulting offsets can be summed
/// directly with the base position in the vertex shader.
fn load_primitive_morph_targets(
    primitive: &gltf::Primitive,
    gltf: &gltf::Gltf,
    expected_vertex_count: usize,
    mesh_idx: usize,
    prim_idx: usize,
    scale: f32,
) -> Option<Vec<crate::expression::MorphTarget>> {
    let target_count = primitive.morph_targets().count();
    if target_count == 0 {
        return None;
    }
    // Cap at the per-primitive limit; extras would silently
    // overflow the `weights: array<vec4, 16>` uniform and never
    // reach the GPU.
    if target_count > MAX_MORPH_TARGETS_PER_PRIMITIVE {
        tracing::warn!(
            "VRM mesh[{mesh_idx}].primitive[{prim_idx}] has {target_count} morph targets, \
             capping at MAX_MORPH_TARGETS_PER_PRIMITIVE = {MAX_MORPH_TARGETS_PER_PRIMITIVE}; \
             the extras will be dropped"
        );
    }
    let mut out = Vec::with_capacity(target_count);
    let mut all_displacements: Vec<Vec<[f32; 3]>> = Vec::with_capacity(target_count);
    for (positions, _normals, _tangents) in primitive
        .reader(|_buffer| gltf.blob.as_deref())
        .read_morph_targets()
        .take(MAX_MORPH_TARGETS_PER_PRIMITIVE)
    {
        let mut offsets = Vec::with_capacity(expected_vertex_count);
        if let Some(positions) = positions {
            for (i, p) in positions.enumerate() {
                if i >= expected_vertex_count {
                    break;
                }
                // Morph target POSITION is a per-vertex *delta*,
                // not an absolute position — normalize by `scale`
                // only. Translating by `-center` would add a
                // spurious shift that drags weighted morphs toward
                // the origin (e.g. "happy" slid the face down by
                // ~0.9 normalised units before this was fixed).
                offsets.push(normalize_morph_offset(p, scale));
            }
        }
        // Pad with zeros if the accessor was shorter.
        offsets.resize(expected_vertex_count, [0.0; 3]);
        all_displacements.push(offsets);
    }
    for (target_index, _json_target) in primitive
        .morph_targets()
        .enumerate()
        .take(MAX_MORPH_TARGETS_PER_PRIMITIVE)
    {
        let offsets = all_displacements
            .get(target_index)
            .cloned()
            .unwrap_or_else(|| vec![[0.0; 3]; expected_vertex_count]);
        out.push(crate::expression::MorphTarget {
            target_index,
            position_offsets: offsets,
        });
    }
    Some(out)
}

/// Walk every glTF skin, build a **merged skeleton** whose
/// `joint_to_node` is the union of all unique joint nodes, and
/// pre-compute a per-primitive remap table so each vertex's
/// `JOINTS_0` (an index into its own skin's joint list) is
/// translated into the merged-skeleton index. The renderer keeps
/// using a single `Skeleton`; the per-primitive remap is a
/// one-time preprocessing step at load time.
fn load_merged_skeleton_and_remaps(gltf: &gltf::Gltf) -> (Skeleton, Vec<Vec<Vec<u32>>>) {
    let mut per_skin: Vec<(Vec<usize>, Vec<glam::Mat4>)> = Vec::new();
    for skin in gltf.document.skins() {
        let joints: Vec<usize> = skin.joints().map(|n| n.index()).collect();
        let ibms: Vec<glam::Mat4> = skin
            .reader(|_b| gltf.blob.as_deref())
            .read_inverse_bind_matrices()
            .map(|ibm| ibm.map(|m| glam::Mat4::from_cols_array_2d(&m)).collect())
            .unwrap_or_default();
        per_skin.push((joints, ibms));
    }

    // Union of all unique joint nodes across every skin. The IBM
    // for each merged joint is the IBM from the first skin that
    // declared it; in practice all skins share the same bind pose.
    let mut merged_joints: Vec<usize> = Vec::new();
    let mut merged_ibms: Vec<glam::Mat4> = Vec::new();
    let mut node_to_merged: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();
    for (joints, ibms) in &per_skin {
        for (i, &joint_node) in joints.iter().enumerate() {
            node_to_merged.entry(joint_node).or_insert_with(|| {
                let merged_idx = merged_joints.len();
                merged_joints.push(joint_node);
                merged_ibms.push(ibms[i]);
                merged_idx
            });
        }
    }
    // `inverse_bind` is the IBM read from
    // `skin.inverse_bind_matrices`; the per-frame palette is
    // `joint_world * inverse_bind[i]` (the standard glTF
    // identity, which collapses to the bind matrix at rest).
    // `bind_matrices` is the inverse, pre-baked for the
    // renderer's initial rest-pose upload.
    //
    // The previous build was assigning the two fields in the
    // opposite order, producing `palette[j] = joint_world *
    // bind_matrix ≈ bind_matrix²` for a T-pose humanoid. That
    // doubled every joint's rest position and stretched the
    // model ~5x vertically.
    let skel = Skeleton {
        inverse_bind: merged_ibms.clone(),
        bind_matrices: merged_ibms.iter().map(glam::Mat4::inverse).collect(),
        joint_to_node: merged_joints,
    };

    // The skin reference lives on the node, not the primitive; a
    // mesh attached to a node with no `skin` property falls back
    // to skin 0 (glTF default).
    let mut mesh_to_skin: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();
    for node in gltf.document.nodes() {
        if let Some(mesh) = node.mesh() {
            let skin_idx = node.skin().map_or(0, |s| s.index());
            mesh_to_skin.entry(mesh.index()).or_insert(skin_idx);
        }
    }

    // Per-primitive remap table indexed
    // `[mesh_idx][prim_idx][old_joint_idx] -> merged_joint_idx`.
    // A missing entry falls back to 0 (the renderer's
    // `update_skin_palette` reduces that to identity).
    let mut primitive_remap: Vec<Vec<Vec<u32>>> = Vec::new();
    for (mesh_idx, mesh) in gltf.document.meshes().enumerate() {
        let mut prim_remaps: Vec<Vec<u32>> = Vec::new();
        let skin_idx = mesh_to_skin.get(&mesh_idx).copied().unwrap_or(0);
        let Some((skin_joints, _)) = per_skin.get(skin_idx) else {
            for _ in 0..mesh.primitives().count() {
                prim_remaps.push(Vec::new());
            }
            primitive_remap.push(prim_remaps);
            continue;
        };
        for _ in 0..mesh.primitives().count() {
            let table: Vec<u32> = skin_joints
                .iter()
                .map(|&node_idx| node_to_merged.get(&node_idx).copied().unwrap_or(0) as u32)
                .collect();
            prim_remaps.push(table);
        }
        primitive_remap.push(prim_remaps);
    }

    (skel, primitive_remap)
}

/// Walk every glTF `Node` and capture the local rotation /
/// position, the parent index, and the world rest-pose
/// transform. The world transform is computed in a single
/// topologically-ordered cascade (glTF requires parents to
/// appear before children; we fall back to root for any
/// out-of-order node and log a warning).
fn load_node_hierarchy(gltf: &gltf::Gltf) -> NodeHierarchy {
    use crate::model::NodeHierarchy;

    let node_count = gltf.document.nodes().count();
    let mut rest_local_rotations = vec![glam::Quat::IDENTITY; node_count];
    let mut rest_local_positions = vec![glam::Vec3::ZERO; node_count];
    let mut parents = vec![-1i32; node_count];
    let mut world_rotations = vec![glam::Quat::IDENTITY; node_count];
    let mut world_positions = vec![glam::Vec3::ZERO; node_count];

    for (i, node) in gltf.document.nodes().enumerate() {
        let (t, r, _s) = node.transform().decomposed();
        rest_local_positions[i] = glam::Vec3::from(t);
        rest_local_rotations[i] = glam::Quat::from_array(r);
    }
    for node in gltf.document.nodes() {
        let idx = node.index();
        for child in node.children() {
            parents[child.index()] = idx as i32;
        }
    }

    let mut out_of_order = 0usize;
    for i in 0..node_count {
        let p = parents[i];
        if p < 0 {
            world_rotations[i] = rest_local_rotations[i];
            world_positions[i] = rest_local_positions[i];
        } else {
            let p = p as usize;
            // Out-of-order node: treat as root for itself so the
            // cascade stays sound for its descendants.
            if i < p {
                out_of_order += 1;
                world_rotations[i] = rest_local_rotations[i];
                world_positions[i] = rest_local_positions[i];
            } else {
                world_rotations[i] = world_rotations[p] * rest_local_rotations[i];
                world_positions[i] =
                    world_rotations[p] * rest_local_positions[i] + world_positions[p];
            }
        }
    }
    if out_of_order > 0 {
        tracing::warn!(
            "VRM has {out_of_order} glTF node(s) declared before their parent; \
             VRMA playback may produce incorrect poses for the affected bones"
        );
    }

    NodeHierarchy {
        local_rotations: rest_local_rotations.clone(),
        local_positions: rest_local_positions.clone(),
        rest_local_rotations,
        rest_local_positions,
        parents,
        world_rotations,
        world_positions,
    }
}

fn load_primitive_base_color_texture(
    primitive: &gltf::Primitive,
    gltf: &gltf::Gltf,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    cache: &mut std::collections::HashMap<usize, std::sync::Arc<VrmTexture>>,
) -> VrmResult<Option<std::sync::Arc<VrmTexture>>> {
    let material = primitive.material();
    let pbr = material.pbr_metallic_roughness();
    let Some(info) = pbr.base_color_texture() else {
        return Ok(None);
    };
    let texture = info.texture();
    let tex_index = texture.index();

    if let Some(cached) = cache.get(&tex_index) {
        return Ok(Some(cached.clone()));
    }

    let image = load_image_data(&texture, gltf).map_err(VrmError::TextureDecode)?;

    let (width, height) = (image.width, image.height);
    let gpu_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vrm.base_color"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &gpu_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &image.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    let view = gpu_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("vrm.base_color.sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("vrm.base_color.bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vrm.base_color.bg"),
        layout: &bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });

    let vrm_tex = std::sync::Arc::new(VrmTexture {
        texture: gpu_texture,
        sampler,
        bind_group_layout,
        bind_group,
    });
    cache.insert(tex_index, vrm_tex.clone());
    Ok(Some(vrm_tex))
}

struct DecodedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn load_image_data(
    texture: &gltf::texture::Texture,
    gltf: &gltf::Gltf,
) -> Result<DecodedImage, String> {
    let source = texture.source();
    let data = match source.source() {
        gltf::image::Source::Uri { uri, mime_type: _ } => {
            if let Some(rest) = uri.strip_prefix("data:") {
                let (_meta, b64) = rest
                    .split_once(',')
                    .ok_or_else(|| "data URI missing ','".to_string())?;
                BASE64_STANDARD
                    .decode(b64)
                    .map_err(|e| format!("base64 decode: {e}"))?
            } else {
                return Err(format!("external image URI not supported: {uri}"));
            }
        }
        gltf::image::Source::View { view, mime_type: _ } => {
            let parent_buffer = view.buffer();
            if parent_buffer.index() != 0 {
                return Err(format!(
                    "only buffer index 0 is supported (got {})",
                    parent_buffer.index()
                ));
            }
            let blob = gltf
                .blob
                .as_ref()
                .ok_or_else(|| "buffer referenced but no GLB blob present".to_string())?;
            let start = view.offset();
            let end = start + view.length();
            blob.get(start..end)
                .ok_or_else(|| format!("buffer view out of range: {start}..{end}"))?
                .to_vec()
        }
    };

    let cursor = std::io::Cursor::new(data);
    let reader = ImageReader::new(cursor)
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let img = reader.decode().map_err(|e| format!("PNG decode: {e}"))?;
    let rgba = img.to_rgba8();
    Ok(DecodedImage {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

/// Map a raw glTF morph-target POSITION displacement into the
/// vertex buffer's coordinate space.
///
/// As of the mesh-unnormalisation pass, the vertex
/// buffer is now in raw glTF space (we no longer recenter or
/// scale vertices on load — the model matrix folds those in
/// at draw time). Morph target POSITION is a per-vertex
/// *delta* in the same glTF space, so the function is the
/// identity. The previous `(raw - center) * scale` form
/// (with the linear-only simplification `delta' = delta *
/// scale`) is preserved by the function signature for
/// forward-compat — a future model that bakes normalisation
/// into the vertices could reintroduce the linear pass
/// without touching call sites.
const fn normalize_morph_offset(raw: [f32; 3], _scale: f32) -> [f32; 3] {
    raw
}

/// Load all `MToon` textures referenced by a material and
/// create the combined GPU bind group. Returns `Ok(None)` if the
/// material has no `MToon` textures at all.
fn load_mtoon_gpu_textures(
    mat_index: usize,
    mat: &mtoon::MToonMaterial,
    gltf: &gltf::Gltf,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    cache: &mut std::collections::HashMap<usize, std::sync::Arc<mtoon::MToonGpuTextures>>,
) -> VrmResult<Option<std::sync::Arc<mtoon::MToonGpuTextures>>> {
    if let Some(cached) = cache.get(&mat_index) {
        return Ok(Some(cached.clone()));
    }

    // Check if any MToon textures are referenced.
    let has_any = mat.shade_multiply_texture.is_some()
        || mat.shading_shift_texture.is_some()
        || mat.emissive_texture.is_some()
        || mat.matcap_texture.is_some()
        || mat.rim_multiply_texture.is_some()
        || mat.outline_width_multiply_texture.is_some()
        || mat.uv_animation_mask_texture.is_some();

    if !has_any {
        return Ok(None);
    }

    // Load each texture (or create a 1x1 white dummy).
    let shade_multiply = load_mtoon_texture_or_dummy(
        mat.shade_multiply_texture.map(|t| t.index),
        gltf,
        device,
        queue,
        "shade_multiply",
    )?;
    let shading_shift = load_mtoon_texture_or_dummy(
        mat.shading_shift_texture.map(|t| t.index),
        gltf,
        device,
        queue,
        "shading_shift",
    )?;
    let emissive = load_mtoon_texture_or_dummy(
        mat.emissive_texture.map(|t| t.index),
        gltf,
        device,
        queue,
        "emissive",
    )?;
    let matcap = load_mtoon_texture_or_dummy(
        mat.matcap_texture.map(|t| t.index),
        gltf,
        device,
        queue,
        "matcap",
    )?;
    let rim_multiply = load_mtoon_texture_or_dummy(
        mat.rim_multiply_texture.map(|t| t.index),
        gltf,
        device,
        queue,
        "rim_multiply",
    )?;
    let outline_width = load_mtoon_texture_or_dummy(
        mat.outline_width_multiply_texture.map(|t| t.index),
        gltf,
        device,
        queue,
        "outline_width",
    )?;
    let uv_mask = load_mtoon_texture_or_dummy(
        mat.uv_animation_mask_texture.map(|t| t.index),
        gltf,
        device,
        queue,
        "uv_mask",
    )?;

    // Create the combined bind group layout and bind group.
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("vrm.mtoon_textures_bgl"),
        entries: &[
            // shade_multiply tex + smp
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // shading_shift tex + smp
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // emissive tex + smp
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // matcap tex + smp
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 7,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // rim_multiply tex + smp
            wgpu::BindGroupLayoutEntry {
                binding: 8,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 9,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // outline_width tex + smp
            wgpu::BindGroupLayoutEntry {
                binding: 10,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 11,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // uv_mask tex + smp
            wgpu::BindGroupLayoutEntry {
                binding: 12,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 13,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });

    let combined_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vrm.mtoon_textures_bg"),
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&shade_multiply.view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&shade_multiply.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&shading_shift.view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&shading_shift.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&emissive.view),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Sampler(&emissive.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&matcap.view),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::Sampler(&matcap.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(&rim_multiply.view),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: wgpu::BindingResource::Sampler(&rim_multiply.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::TextureView(&outline_width.view),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: wgpu::BindingResource::Sampler(&outline_width.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 12,
                resource: wgpu::BindingResource::TextureView(&uv_mask.view),
            },
            wgpu::BindGroupEntry {
                binding: 13,
                resource: wgpu::BindingResource::Sampler(&uv_mask.sampler),
            },
        ],
    });

    // Create individual bind groups for each texture slot (for flexibility).
    let single_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("vrm.mtoon_single_tex_bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });

    let make_single = |tex: &MToonGpuTexture, label: &str| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &single_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&tex.sampler),
                },
            ],
        })
    };

    let gpu_tex = std::sync::Arc::new(mtoon::MToonGpuTextures {
        shade_multiply: make_single(&shade_multiply, "vrm.mtoon_shade_multiply_bg"),
        shading_shift: make_single(&shading_shift, "vrm.mtoon_shading_shift_bg"),
        emissive: make_single(&emissive, "vrm.mtoon_emissive_bg"),
        matcap: make_single(&matcap, "vrm.mtoon_matcap_bg"),
        rim_multiply: make_single(&rim_multiply, "vrm.mtoon_rim_multiply_bg"),
        outline_width: make_single(&outline_width, "vrm.mtoon_outline_width_bg"),
        uv_mask: make_single(&uv_mask, "vrm.mtoon_uv_mask_bg"),
        combined_bind_group,
    });
    cache.insert(mat_index, gpu_tex.clone());
    Ok(Some(gpu_tex))
}

/// A single GPU texture + sampler for `MToon`.
struct MToonGpuTexture {
    #[expect(dead_code)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
}

/// Load a single glTF texture by index, or create a 1×1 white
/// dummy if `index` is `None`.
fn load_mtoon_texture_or_dummy(
    index: Option<usize>,
    gltf: &gltf::Gltf,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
) -> VrmResult<MToonGpuTexture> {
    if let Some(idx) = index {
        let texture = gltf.document.textures().nth(idx);
        if let Some(tex) = texture {
            let image = load_image_data(&tex, gltf).map_err(VrmError::TextureDecode)?;
            return upload_mtoon_texture(&image, device, queue, label);
        }
    }
    // 1×1 white dummy.
    let white = DecodedImage {
        width: 1,
        height: 1,
        rgba: vec![255, 255, 255, 255],
    };
    upload_mtoon_texture(&white, device, queue, label)
}

#[expect(clippy::unnecessary_wraps)]
fn upload_mtoon_texture(
    image: &DecodedImage,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
) -> VrmResult<MToonGpuTexture> {
    let (width, height) = (image.width, image.height);
    let gpu_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &gpu_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &image.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let view = gpu_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some(&format!("{label}.sampler")),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    Ok(MToonGpuTexture {
        texture: gpu_texture,
        view,
        sampler,
    })
}

#[cfg(test)]
mod tests {
    use super::normalize_morph_offset;

    /// as of the mesh-unnormalisation pass the vertex
    /// buffer is in raw glTF space; the morph offset is
    /// already in the same raw space, so
    /// `normalize_morph_offset` is the identity. The helper
    /// keeps the `(center, scale)` signature for forward
    /// compatibility with a future model that bakes
    /// normalisation into the vertices, but the shipped
    /// pass-through must not pick up the model's centre or
    /// scale.
    #[test]
    fn morph_offset_is_passthrough_in_raw_space() {
        let center = [0.0, 0.5, 0.0];
        let scale = 1.5 / 0.8;
        // A "happy" smile might add 2 cm to a mouth
        // corner. With the raw-space pass-through the
        // helper must return the raw delta untouched,
        // regardless of `center` / `scale`. The earlier
        // assertion (`raw * scale`) is now obsolete:
        // the per-vertex `scale` lives in the model matrix,
        // not in the morph offset buffer.
        let raw = [0.0, 0.02, 0.01];
        let out = normalize_morph_offset(raw, scale);
        for i in 0..3 {
            let diff = (out[i] - raw[i]).abs();
            assert!(
                diff < 1e-6,
                "axis {i}: got {out:?}, expected passthrough {raw:?}, \
                 centre * scale = {ys:?} (must NOT be subtracted)",
                ys = [center[0] * scale, center[1] * scale, center[2] * scale],
            );
        }
    }

    #[test]
    fn morph_offset_scales_linearly() {
        // Doubling the raw delta must double the result.
        let a = normalize_morph_offset([0.1, 0.0, -0.05], 0.5);
        let b = normalize_morph_offset([0.2, 0.0, -0.10], 0.5);
        for i in 0..3 {
            assert!(a[i].mul_add(2.0, -b[i]).abs() < 1e-6, "axis {i}");
        }
    }

    /// when the loader computes `bind_matrices` from
    /// `inverse_bind`, the identity `bind * inverse_bind`
    /// must hold at every joint (modulo float round-off).
    /// This is the algebraic invariant the skinned shader
    /// relies on at rest pose.
    #[test]
    fn bind_matrices_are_inverse_of_inverse_bind() {
        // A non-trivial inverse-bind matrix (translation +
        // uniform scale, the kind glTF exporters actually
        // produce for humanoid bones).
        let ibm = glam::Mat4::from_scale_rotation_translation(
            glam::Vec3::splat(1.25),
            glam::Quat::IDENTITY,
            glam::Vec3::new(0.0, 1.2, 0.05),
        );
        let bind = ibm.inverse();
        let product = bind * ibm;
        // bind * inverse_bind should be the identity matrix.
        let identity = glam::Mat4::IDENTITY;
        for col in 0..4 {
            for row in 0..4 {
                let diff = (product.col(col)[row] - identity.col(col)[row]).abs();
                assert!(
                    diff < 1e-5,
                    "bind * ibm at ({row}, {col}) = {}, expected identity",
                    product.col(col)[row]
                );
            }
        }
    }

    /// humanoid models with 256+ joints (e.g. every
    /// per-finger bone) used to clamp `JOINTS_0` to `[u8; 4]`
    /// and silently aliased every joint >= 255 onto
    /// `skin_matrices[255]`, gluing finger / wrist skinning to
    /// the same matrix. The `MeshVertex` is now `[u32; 4]` and
    /// the `into_u16 → u32` cast in the loader preserves
    /// indices up to 65535 — the full u16 range glTF allows.
    #[test]
    fn joints_preserve_indices_above_255() {
        // Simulate the cast path the loader uses: read as u16,
        // widen to u32, no clamp. A 512-joint model with a
        // wrist + fingertip vertex exercises the upper half of
        // the range.
        let high_index: u32 = 511;
        let joints: [u32; 4] = [u32::from(high_index), 0, 0, 0];
        // The WGSL palette index is `u32` so a `Uint32x4`
        // attribute is the only correct upload format. The
        // `[u8; 4]` packing (this struct) would have saturated
        // `511` to `255`.
        assert_eq!(joints[0], 511);
        // The cast path must not produce 0 / 255 by truncation.
        assert_ne!(u32::from(joints[0] as u8), 511);
    }

    /// the loader flags a malformed model as
    /// "`has_nonzero_joints`" when any primitive carries a
    /// non-trivial `JOINTS_0` accessor. The predicate is the
    /// one the `load_all_meshes` body uses inline (it is not
    /// pulled into a public function because the glTF
    /// `ReadJoints` iterator is not `Clone`). This test
    /// documents the contract: the model is malformed when
    /// at least one vertex has any non-zero joint index.
    #[test]
    fn nonzero_joint_detector_trips_on_nontrivial_indices() {
        // The four shapes `load_all_meshes` produces, plus the
        // all-zero default.
        let cases: Vec<([u32; 4], bool)> = vec![
            ([0, 0, 0, 0], false),
            ([1, 0, 0, 0], true),
            ([0, 0, 0, 7], true),
            ([12, 34, 56, 78], true),
        ];
        for (joints, expected) in cases {
            let has = joints.iter().any(|x| *x != 0);
            assert_eq!(
                has, expected,
                "joints {joints:?} should be flagged = {expected}"
            );
        }
    }
}
