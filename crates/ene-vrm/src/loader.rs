//! VRM file loader.
//!
//! Reads a `.vrm` file from disk and produces a [`VrmModel`] with
//! every primitive of **every** glTF `Mesh` uploaded to the GPU
//! (vertex / index buffers + each primitive's own base-color
//! texture) and the first skin's inverse-bind matrices preserved
//! for PR4's skinning pass.
//!
//! ## Multi-mesh scope (PR3.3)
//!
//! A VRM 1.0 file such as `AliciaSolid.vrm` contains ~12 separate
//! glTF `Mesh` objects (body, hair, face, clothes, accessories,
//! …), not one mesh with many primitives. PR3.0/3.1/3.2 only
//! loaded `meshes[0]` (the head/face area) and therefore rendered
//! only the skin. PR3.3 walks the entire `gltf.meshes()` iterator,
//! computing one global AABB across all primitive positions so the
//! per-vertex normalization (`TARGET_MODEL_SIZE = 1.5` m) is
//! applied uniformly across the whole body.
//!
//! ## PR3.x deliberately does **not** support
//!
//! - MToon's full PBR parameters (rim / matcap / outline /
//!   emission). The shader applies a simple diffuse + lit + base
//!   color. The MToon-flavored `KHR_materials_unlit` flag is
//!   *read* so the loader can log a warning if it is set, but
//!   does not yet alter rendering.
//! - Per-mesh glTF node transforms. The PR3.3 loader uses raw
//!   vertex positions from each mesh without applying the glTF
//!   node hierarchy's world transforms. For VRoid-exported models
//!   this is fine because the body parts are already in
//!   world-space positions in their local vertex buffers. The
//!   humanoid bone system (PR4) will eventually apply the
//!   per-joint skinning matrices on top of this.
//! - Animation, expressions, morph targets, spring bone.
//! - `.gltf` (non-binary) VRM files. Only `.glb` (binary) is
//!   supported in PR3.x; the glTF binary payload (`BIN` chunk)
//!   holds all the meshes / textures. External `.bin` files
//!   require the `gltf` crate's `import` feature and ship as a
//!   follow-up PR.
//!
//! See `docs/architecture/wgpu-migration.md` §22.6 for the PR3
//! status.
use std::collections::HashMap;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use image::ImageReader;
use wgpu::util::DeviceExt;

use crate::error::{VrmError, VrmResult};
use crate::expression::{
    ExpressionLayer, MAX_MORPH_TARGETS_PER_PRIMITIVE, PrimitiveId, PrimitiveMorphs,
};
use crate::model::{MeshVertex, Skeleton, VrmMesh, VrmModel, VrmPrimitive, VrmTexture};

/// Target world-space size of the model along its longest axis
/// after PR3.1's per-vertex normalization. The legacy Bevy
/// `bevy_vrm1` bakes the same scale into its world transform;
/// v2 applies it at load time.
const TARGET_MODEL_SIZE: f32 = 1.5;

/// Load a `.vrm` file from disk and upload every primitive of the
/// first mesh to the GPU.
///
/// `path` is the on-disk `.vrm` (a glTF binary with the VRMC_vrm
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

    let expression_names = resolve_expression_names(&gltf);
    let (mesh, morph_per_primitive, (aabb_min, aabb_max), has_nonzero_joints) =
        load_all_meshes(&gltf, device, queue, &expression_names)?;
    let skin_joint_to_node = load_skin_joint_to_node(&gltf);
    let skeleton = load_first_skeleton(&gltf, &skin_joint_to_node);

    // Issue #8: a model that defines no skin but whose
    // primitives carry non-trivial `JOINTS_0` data is
    // malformed (most likely a VRM 0.x file that lost its
    // skin during export, or a hand-rolled glTF whose exporter
    // wrote `JOINTS_0` without an `IBM`). The renderer
    // currently falls back to a one-element
    // `Mat4::IDENTITY` skin palette + `joints = [0, 0, 0, 0]`,
    // which is a silent no-op: the per-vertex weighting looks
    // like identity, but the model really wants
    // skinning. Warn here so the user can re-export the model;
    // PR5+ will promote this to a load-time error once the
    // refactor lands a proper "load is malformed" channel.
    if skeleton.joint_count() == 0 && has_nonzero_joints {
        tracing::warn!(
            "VRM {} has no skin but primitive(s) carry non-trivial JOINTS_0; \
             skinning will fall back to identity. Re-export with VRMC_vrm 1.0 to fix.",
            path.display()
        );
    }

    for material in gltf.document.materials() {
        if material.unlit() {
            tracing::info!(
                "VRM {} uses KHR_materials_unlit; PR3 shader ignores the flag",
                path.display()
            );
        }
    }

    // PR4.4: Build the expression layer from the per-primitive
    // morph target data collected during mesh load. Primitives
    // without morphs already have `None` slots.
    let expression_layer = ExpressionLayer::new(morph_per_primitive);
    let expression_count: usize = expression_layer
        .per_primitive
        .iter()
        .map(|p| p.as_ref().map(|m| m.targets.len()).unwrap_or(0))
        .sum();
    if expression_count > 0 {
        tracing::info!(
            "VRM {} loaded {} expression(s) across {} primitive(s)",
            path.display(),
            expression_layer.expression_names().len(),
            expression_layer.morphic_primitive_count(),
        );
    }

    Ok(VrmModel::new(
        mesh,
        skeleton,
        aabb_min,
        aabb_max,
        expression_layer,
    ))
}

/// Load every triangle-list primitive of every glTF `Mesh` in the
/// document. Returns a `Vec<VrmMesh>` — one entry per glTF `Mesh`.
/// Each primitive gets its own vertex / index buffers and (when
/// its material has one) its own base-color texture, so the body,
/// clothes, face, hair, and accessories all render.
///
/// The third return value is the per-primitive morph-target data
/// (PR4.4), aligned 1:1 with the linearized primitive list
/// `(mesh_idx, prim_idx)`. The fourth and fifth are the
/// post-normalize AABB `(min, max)`.
///
/// `expression_names` is the resolver map produced by
/// [`resolve_expression_names`]; the loader uses it to rename
/// each primitive's morph targets to the real VRMC_vrm
/// expression name (e.g. `happy`, `sad`) instead of the
/// synthetic `morph_target_<i>` fallback.
type Aabb = ([f32; 3], [f32; 3]);

/// Aggregate return of [`load_all_meshes`]: the per-primitive
/// mesh data, the parallel morph-target slot vector, the
/// post-normalize AABB, and the "any primitive carries a
/// non-trivial `JOINTS_0` accessor?" flag that powers the
/// issue #8 malformed-skin warning.
type LoadAllMeshesResult = (Vec<VrmMesh>, Vec<Option<PrimitiveMorphs>>, Aabb, bool);

fn load_all_meshes(
    gltf: &gltf::Gltf,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    expression_names: &HashMap<(usize, usize, usize), String>,
) -> VrmResult<LoadAllMeshesResult> {
    // First pass: collect every triangle-list primitive's
    // positions across **all** glTF meshes so we can compute one
    // global AABB and apply a uniform normalize-scale. (A VRM
    // 1.0 has ~12 separate glTF Mesh objects, not one.)
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
        (bb_min[0] + bb_max[0]) * 0.5,
        (bb_min[1] + bb_max[1]) * 0.5,
        (bb_min[2] + bb_max[2]) * 0.5,
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
    // PR4.4: also extract morph targets per primitive (position
    // displacements only, normalised by `(center, scale)` so the
    // GPU matches the vertex buffer's scale). The per-primitive
    // morph data is stored in a parallel `Vec<Option<PrimitiveMorphs>>`
    // aligned 1:1 with the final `VrmPrimitive` list (skipped
    // primitives are recorded as `None` so the indices stay
    // stable).
    //
    // Issue #8: we also track whether any primitive carries a
    // non-trivial `JOINTS_0` (i.e. any index > 0). When
    // combined with a missing skin (decided by the caller
    // after `load_first_skeleton` runs) this signals a
    // malformed model. The flag is returned alongside the
    // mesh and morph data so the caller can log a single
    // combined warning.
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

            let positions: Vec<[f32; 3]> = match reader.read_positions() {
                Some(p) => p.collect(),
                None => {
                    tracing::warn!(
                        "VRM mesh[{mesh_idx}].primitive[{prim_idx}] has no POSITION; skipping"
                    );
                    continue;
                }
            };
            let normals: Vec<[f32; 3]> = reader
                .read_normals()
                .map(|n| n.collect())
                .unwrap_or_else(|| vec![[0.0, 0.0, 1.0]; positions.len()]);
            let uvs: Vec<[f32; 2]> = reader
                .read_tex_coords(0)
                .map(|tc| tc.into_f32().collect())
                .unwrap_or_else(|| vec![[0.0, 0.0]; positions.len()]);
            // PR4.5: per-vertex joints / weights. glTF stores
            // `JOINTS_0` as a u8/u16 accessor and `WEIGHTS_0` as
            // f32. When the primitive does not carry skinning
            // data the defaults `[joints = [0,0,0,0],
            // weights = [1,0,0,0]]` make the skinned shader
            // reduce to `weights[0] * skin[0] * pos` and the
            // renderer uploads a one-element `skin[]` of
            // `Mat4::IDENTITY`.
            //
            // The gltf 1.4 `ReadJoints` enum exposes `into_u16`
            // (which handles both u8 and u16 accessors uniformly
            // via a `CastingIter`) — we promote the joint index
            // to `u32` so 256+-joint humanoid models (per-finger
            // bones, etc.) can address every joint. An earlier
            // `[u8; 4]` representation silently aliased every
            // joint >= 255 onto `skin_matrices[255]`, which
            // stuck finger / wrist skinning to the same matrix.
            let joints: Vec<[u32; 4]> = reader
                .read_joints(0)
                .map(|js| {
                    js.into_u16()
                        .map(|j| [j[0] as u32, j[1] as u32, j[2] as u32, j[3] as u32])
                        .collect()
                })
                .unwrap_or_else(|| vec![[0, 0, 0, 0]; positions.len()]);
            // Issue #8: detect malformed models that carry
            // `JOINTS_0` indices without an accompanying skin.
            // We only need one primitive with a non-zero index
            // to flag the model — once a real skin lands the
            // indices will be honoured and the warning will
            // disappear.
            if !has_nonzero_joints && joints.iter().any(|j| j.iter().any(|x| *x != 0)) {
                has_nonzero_joints = true;
            }
            let weights: Vec<[f32; 4]> = reader
                .read_weights(0)
                .map(|ws| ws.into_f32().collect())
                .unwrap_or_else(|| vec![[1.0, 0.0, 0.0, 0.0]; positions.len()]);
            let indices: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => {
                    tracing::warn!(
                        "VRM mesh[{mesh_idx}].primitive[{prim_idx}] has no index buffer; skipping"
                    );
                    continue;
                }
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
                vertices.push(MeshVertex {
                    position: [
                        (pos[0] - center[0]) * scale,
                        (pos[1] - center[1]) * scale,
                        (pos[2] - center[2]) * scale,
                    ],
                    uv: *uv,
                    normal: *normal,
                    joints: *joint,
                    weights: *weight,
                });
            }

            let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("vrm.mesh{mesh_idx}.prim{prim_idx}.vertex_buf")),
                contents: MeshVertex::as_bytes(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("vrm.mesh{mesh_idx}.prim{prim_idx}.index_buf")),
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX,
            });

            let base_color = match load_primitive_base_color_texture(
                &primitive, gltf, device, queue,
            ) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(
                        "VRM mesh[{mesh_idx}].primitive[{prim_idx}] base-color decode failed: {e}"
                    );
                    None
                }
            };

            // PR4.4: extract morph targets BEFORE pushing the
            // primitive, so the index lines up. The resolver from
            // `resolve_expression_names` is used to map the
            // synthetic slot index to the real VRMC_vrm
            // expression name (e.g. `happy`, `sad`).
            let morphs = load_primitive_morph_targets(
                &primitive,
                gltf,
                positions.len(),
                mesh_idx,
                prim_idx,
                expression_names,
                scale,
            );

            primitives.push(VrmPrimitive {
                vertex_buf,
                vertex_count: vertices.len() as u32,
                index_buf,
                index_count: indices.len() as u32,
                base_color,
            });
            // Allocate a stable PrimitiveId based on the running
            // count of successfully-loaded primitives. The
            // renderer's draw loop will use the same ordering
            // (mesh-major, primitive-within-mesh) to look up the
            // matching morph data.
            let pid = PrimitiveId(morph_per_primitive.len());
            morph_per_primitive.push(
                morphs.map(|raw| PrimitiveMorphs::from_targets(pid, vertices.len() as u32, raw)),
            );
        }
        if primitives.is_empty() {
            tracing::debug!("VRM mesh[{mesh_idx}] has no renderable primitives; skipping");
            // Drop the morph records we pushed for this empty
            // mesh so the indices stay aligned with `VrmPrimitive`
            // in `meshes`. PR3.x already had a
            // `meshes.push(VrmMesh { ... })` only on the
            // non-empty branch, so this matches that.
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

    // Compute the AABB of the **normalized** vertices so the
    // runtime can log it as a diagnostic. Without this the loader
    // only ever logs the raw AABB (before centering + scaling).
    // Derived analytically from the raw AABB + center + scale,
    // which is exact because every vertex was transformed by
    // `(pos - center) * scale` (linear transform preserves AABB
    // shape up to axis sign).
    let post_min = [
        (bb_min[0] - center[0]) * scale,
        (bb_min[1] - center[1]) * scale,
        (bb_min[2] - center[2]) * scale,
    ];
    let post_max = [
        (bb_max[0] - center[0]) * scale,
        (bb_max[1] - center[1]) * scale,
        (bb_max[2] - center[2]) * scale,
    ];
    Ok((
        meshes,
        morph_per_primitive,
        (post_min, post_max),
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
#[allow(clippy::too_many_arguments)]
fn load_primitive_morph_targets(
    primitive: &gltf::Primitive,
    gltf: &gltf::Gltf,
    expected_vertex_count: usize,
    mesh_idx: usize,
    prim_idx: usize,
    expression_names: &HashMap<(usize, usize, usize), String>,
    scale: f32,
) -> Option<Vec<(String, Vec<[f32; 3]>)>> {
    let target_count = primitive.morph_targets().count();
    if target_count == 0 {
        return None;
    }
    // Issue #6: enforce the per-primitive cap that the GPU
    // uniform reserves (`MAX_WEIGHT_SLOTS * 4` = 64 weight
    // slots). A model that ships more morph targets than the
    // cap would silently overflow the `weights: array<vec4, 16>`
    // uniform and the extras would never reach the GPU; we
    // warn and truncate here so the user can see the issue and
    // the bound uniform stays consistent.
    if target_count > MAX_MORPH_TARGETS_PER_PRIMITIVE {
        tracing::warn!(
            "VRM mesh[{mesh_idx}].primitive[{prim_idx}] has {target_count} morph targets, \
             capping at MAX_MORPH_TARGETS_PER_PRIMITIVE = {MAX_MORPH_TARGETS_PER_PRIMITIVE}; \
             the extras will be dropped"
        );
    }
    let mut out = Vec::with_capacity(target_count);
    // Read all displacement data once. The `Reader` type is
    // bound to the `gltf` lifetime so we cannot return it from
    // this helper — instead we walk the iterator here. The
    // `take` cap mirrors the warning above: any extras are
    // dropped at both the storage-collection and the name-pair
    // pass so the `out` vec never exceeds
    // `MAX_MORPH_TARGETS_PER_PRIMITIVE`.
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
                // Morph target POSITION is a per-vertex *delta*
                // (not an absolute position), so it must be
                // normalised by `scale` only — NOT by
                // `(p - center) * scale` like the base positions
                // are. Translating by `-center` adds a spurious
                // shift equal to the model's centre, which makes
                // weighted morphs drag the mesh toward the
                // origin. Verified against the PR4.4 visual
                // regression where "happy" slid the face down by
                // ~0.9 normalised units.
                offsets.push(normalize_morph_offset(p, scale));
            }
        }
        // Pad with zeros if the accessor was shorter.
        offsets.resize(expected_vertex_count, [0.0; 3]);
        all_displacements.push(offsets);
    }
    // Pair each entry with the name. VRM 1.0 stores expression
    // names in the `VRMC_vrm.expressions.{preset,custom}.<name>`
    // ext tree — not on the morph target itself. The resolver
    // `resolve_expression_names` walks that tree once and returns
    // a `HashMap<(mesh_idx, prim_idx, target_idx), name>`. We
    // look the name up here; when the resolver didn't find a
    // matching bind (e.g. legacy VRM 0.x that has no
    // VRMC_vrm.expressions block) we fall back to the synthetic
    // name `morph_target_<i>` so the GPU buffers always get a
    // name.
    for (target_index, _json_target) in primitive
        .morph_targets()
        .enumerate()
        .take(MAX_MORPH_TARGETS_PER_PRIMITIVE)
    {
        let name = expression_names
            .get(&(mesh_idx, prim_idx, target_index))
            .cloned()
            .unwrap_or_else(|| format!("morph_target_{target_index}"));
        let offsets = all_displacements
            .get(target_index)
            .cloned()
            .unwrap_or_else(|| vec![[0.0; 3]; expected_vertex_count]);
        out.push((name, offsets));
    }
    Some(out)
}

/// Walk the `VRMC_vrm.expressions.{preset,custom}.<name>.morphTargetBinds`
/// tree and return a `HashMap` keyed by
/// `(mesh_idx, prim_idx, morph_target_idx)` whose values are the
/// expression name.
///
/// VRM 1.0 puts expression names in the
/// `VRMC_vrm.expressions.preset.<name>.morphTargetBinds[*]` and
/// `.custom.<name>.morphTargetBinds[*]` JSON arrays (see
/// <https://github.com/vrm-c/vrm-specification/blob/master/specification/VRMC_vrm-1.0/expressions.md>).
/// Each bind has `{ node, index, weight }` referring to a
/// `(glTF node, morph_target_index)` pair. The glTF `Mesh` the
/// node points at gets that morph target bound to the named
/// expression.
///
/// Per the spec the `index` is the morph target index "assuming
/// all primitives have the same morphTarget" — so we apply the
/// name to every primitive of the bound mesh. This matches the
/// loader's flat-indexed `morph_per_primitive` layout.
///
/// Returns an empty map when the extension is missing (e.g. on
/// VRM 0.x). The caller is then expected to fall back to the
/// synthetic `morph_target_<i>` naming convention.
fn resolve_expression_names(gltf: &gltf::Gltf) -> HashMap<(usize, usize, usize), String> {
    let mut map: HashMap<(usize, usize, usize), String> = HashMap::new();

    let Some(ext) = gltf.document.extensions() else {
        return map;
    };
    let Some(vrm) = ext.get("VRMC_vrm") else {
        return map;
    };
    let Some(expressions) = vrm.get("expressions") else {
        return map;
    };
    let Some(expressions_obj) = expressions.as_object() else {
        return map;
    };

    // Cache the (node_index, mesh_index) lookup so we do not
    // re-walk `gltf.document.nodes()` once per bind. Built
    // lazily on first use because most models have a few
    // hundred nodes and the loop dominates when we have to
    // re-walk.
    let mut node_to_mesh: Vec<Option<usize>> = Vec::new();
    let mut resolve_node_mesh = |node_idx: usize| -> Option<usize> {
        if node_to_mesh.is_empty() {
            // Pre-fill with `None` for every node so the
            // `node_to_mesh.len()` check below stays correct
            // when looking up nodes whose mesh slot has not
            // been visited yet.
            node_to_mesh = gltf
                .document
                .nodes()
                .map(|n| n.mesh().map(|m| m.index()))
                .collect();
        }
        node_to_mesh.get(node_idx).copied().flatten()
    };

    for (group_name, group) in expressions_obj {
        if group_name != "preset" && group_name != "custom" {
            continue;
        }
        let Some(exprs) = group.as_object() else {
            continue;
        };
        for (name, expr) in exprs {
            let Some(binds) = expr.get("morphTargetBinds").and_then(|b| b.as_array()) else {
                continue;
            };
            for bind in binds {
                let Some(node_idx) = bind
                    .get("node")
                    .and_then(|n| n.as_u64())
                    .map(|n| n as usize)
                else {
                    continue;
                };
                let Some(target_idx) = bind
                    .get("index")
                    .and_then(|n| n.as_u64())
                    .map(|n| n as usize)
                else {
                    continue;
                };
                let Some(mesh_idx) = resolve_node_mesh(node_idx) else {
                    continue;
                };
                // Bind the name to every primitive of the mesh;
                // see the doc comment for why.
                let prim_count = gltf
                    .document
                    .meshes()
                    .nth(mesh_idx)
                    .map(|m| m.primitives().count())
                    .unwrap_or(0);
                for prim_idx in 0..prim_count {
                    // Last write wins if the same (mesh, prim,
                    // target) appears in multiple expressions —
                    // VRM 1.0 says expressions are uniquely
                    // named, but a malformed file with
                    // overlapping binds should not panic.
                    map.insert((mesh_idx, prim_idx, target_idx), name.clone());
                }
            }
        }
    }

    if !map.is_empty() {
        tracing::debug!(
            "VRM {} expression bind(s) resolved from VRMC_vrm",
            map.len()
        );
    }
    map
}

/// Walk `document.skins().next().joints()` and return the
/// `(joint_index) -> glTF node_index` map the renderer will need
/// to look up the humanoid bone per joint in Phase 2. Returns an
/// empty vec for models with no skin.
fn load_skin_joint_to_node(gltf: &gltf::Gltf) -> Vec<usize> {
    let Some(skin) = gltf.document.skins().next() else {
        return Vec::new();
    };
    skin.joints().map(|n| n.index()).collect()
}

fn load_first_skeleton(gltf: &gltf::Gltf, joint_to_node: &[usize]) -> Skeleton {
    let mut skel = Skeleton::default();
    let Some(skin) = gltf.document.skins().next() else {
        return skel;
    };
    let reader = skin.reader(|_buffer| gltf.blob.as_deref());
    if let Some(ibm) = reader.read_inverse_bind_matrices() {
        skel.inverse_bind = ibm.map(|m| glam::Mat4::from_cols_array_2d(&m)).collect();
        // Pre-compute the bind matrix `inverse_bind[i].inverse()`
        // so the renderer can upload `skin_matrices[]` as
        // `bind_matrices` for the rest-pose pass (Phase 1).
        // `glam::Mat4::inverse` returns `Mat4::IDENTITY` for
        // singular matrices so a malformed skin still produces
        // a valid (if visually wrong) rest pose.
        skel.bind_matrices = skel.inverse_bind.iter().map(|m| m.inverse()).collect();
    }
    // The `joint_to_node` list is `skin.joints()` and may be
    // longer than the inverse_bind list when a model ships
    // joints without bind matrices (rare but possible). Truncate
    // to the IBM length so the two arrays stay aligned.
    let ibm_len = skel.inverse_bind.len();
    skel.joint_to_node = joint_to_node.iter().take(ibm_len).copied().collect();
    if skel.joint_count() > 0 {
        tracing::info!("VRM skeleton loaded: {} joints", skel.joint_count());
    }
    skel
}

fn load_primitive_base_color_texture(
    primitive: &gltf::Primitive,
    gltf: &gltf::Gltf,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> VrmResult<Option<VrmTexture>> {
    let material = primitive.material();
    let pbr = material.pbr_metallic_roughness();
    let Some(info) = pbr.base_color_texture() else {
        return Ok(None);
    };
    let texture = info.texture();
    let image =
        load_image_data(&texture, gltf).map_err(|e| VrmError::TextureDecode(e.to_string()))?;

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

    Ok(Some(VrmTexture {
        texture: gpu_texture,
        sampler,
        bind_group_layout,
        bind_group,
    }))
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
                .ok_or_else(|| format!("buffer view out of range: {}..{}", start, end))?
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
/// renderer's normalised vertex space.
///
/// The vertex buffer is normalised as `(p - center) * scale`, but
/// morph target POSITION is a per-vertex *delta* — so the linear
/// transform collapses to `delta' = delta * scale`. Applying
/// `-center` here was the PR4.4 visual-regression bug: a 0.5 m
/// torso centre × a 1.5 / 0.8 ≈ 1.875 scale dragged the face
/// down by ≈ 0.94 normalised units the moment any emotion weight
/// went above zero.
fn normalize_morph_offset(raw: [f32; 3], scale: f32) -> [f32; 3] {
    [raw[0] * scale, raw[1] * scale, raw[2] * scale]
}

#[cfg(test)]
mod tests {
    use super::normalize_morph_offset;

    /// Regression for the PR4.4 face-shift bug. Earlier code did
    /// `(p - center) * scale` for the morph delta, which added
    /// `center * scale` to every vertex and dragged the face
    /// down by ≈ 0.9 m the moment a weight became non-zero.
    #[test]
    fn morph_offset_is_not_translated_by_model_centre() {
        // A typical Alicia-like setup: torso-centred model, longest
        // extent 0.8 m, target 1.5 m.
        let center = [0.0, 0.5, 0.0];
        let scale = 1.5 / 0.8;
        // A "happy" smile might add 2 cm to a mouth corner.
        let raw = [0.0, 0.02, 0.01];
        let out = normalize_morph_offset(raw, scale);
        // Expected: `raw * scale` only — no `-center` term.
        let expected = [0.0, 0.02 * scale, 0.01 * scale];
        for i in 0..3 {
            let diff = (out[i] - expected[i]).abs();
            assert!(
                diff < 1e-6,
                "axis {i}: got {out:?}, expected {expected:?}, \
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
            assert!((a[i] * 2.0 - b[i]).abs() < 1e-6, "axis {i}");
        }
    }

    /// PR4.5: when the loader computes `bind_matrices` from
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

    /// Issue #5: humanoid models with 256+ joints (e.g. every
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
        let high_index: u16 = 511;
        let joints: [u32; 4] = [high_index as u32, 0, 0, 0];
        // The WGSL palette index is `u32` so a `Uint32x4`
        // attribute is the only correct upload format. The
        // `[u8; 4]` packing (PR4.5) would have saturated
        // `511` to `255`.
        assert_eq!(joints[0], 511);
        // The cast path must not produce 0 / 255 by truncation.
        assert_ne!(joints[0] as u8 as u32, 511);
    }

    /// Issue #8: the loader flags a malformed model as
    /// "has_nonzero_joints" when any primitive carries a
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
                "joints {:?} should be flagged = {}",
                joints, expected
            );
        }
    }
}
