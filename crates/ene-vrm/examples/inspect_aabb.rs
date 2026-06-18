//! Diagnostic: dump skin, humanoid, and AABB information
//! for a `.vrm` file. The dump is what the runtime's
//! skinning + VRMA playback pipeline consumes, so a
//! quick scan reveals the cause of any "model looks
//! wrong at rest" symptom without spinning up a wgpu
//! device.
//!
//! VRM 1.0 is a zip containing a `.glb` payload; we extract
//! the inner glb bytes then walk the glb header manually so
//! the example can stay inside the same `default-features =
//! false` dependency surface as the lib itself.
//!
//! Usage:
//!   cargo run -p ene-vrm --example inspect_aabb -- <path/to/model.vrm>

use std::path::PathBuf;

fn main() {
    let path: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("../assets/characters/Alicia/AliciaSolid.vrm"));
    println!("Reading {path:?}");

    let vrm_bytes = std::fs::read(&path).expect("read vrm file");

    let glb_bytes = extract_first_glb(&vrm_bytes).expect("extract .glb from .vrm");

    if glb_bytes.len() < 12 {
        eprintln!("glb payload too small: {} bytes", glb_bytes.len());
        return;
    }
    let magic = &glb_bytes[0..4];
    if magic != b"glTF" {
        eprintln!("not a .glb (magic = {:?})", magic);
        return;
    }
    let total_len = u32::from_le_bytes(glb_bytes[8..12].try_into().unwrap()) as usize;
    println!("glb total length = {total_len} bytes");

    let mut cursor = 12usize;
    let mut json: Option<Vec<u8>> = None;
    let mut bin: Option<Vec<u8>> = None;
    while cursor + 8 <= glb_bytes.len() {
        let chunk_len =
            u32::from_le_bytes(glb_bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
        let chunk_type = &glb_bytes[cursor + 4..cursor + 8];
        let payload_start = cursor + 8;
        let payload_end = payload_start + chunk_len;
        if payload_end > glb_bytes.len() {
            eprintln!("chunk overruns glb at {cursor}: len={chunk_len}");
            break;
        }
        let payload = &glb_bytes[payload_start..payload_end];
        match chunk_type {
            b"JSON" => json = Some(payload.to_vec()),
            b"BIN\0" => bin = Some(payload.to_vec()),
            _ => {
                eprintln!("unknown chunk type {:?} at offset {cursor}", chunk_type);
            }
        }
        let padded = (chunk_len + 3) & !3;
        cursor = payload_start + padded;
    }

    let json = json.expect("missing JSON chunk");
    let bin = bin.unwrap_or_default();

    println!(
        "json chunk = {} bytes, bin chunk = {} bytes",
        json.len(),
        bin.len()
    );

    let gltf = gltf::Gltf::from_slice(&json).expect("parse glTF JSON");
    let blob = if bin.is_empty() {
        None
    } else {
        Some(bin.as_slice())
    };

    // ---- AABB / center / normalize_scale (PR4.18) ----
    let (raw_aabb, center, normalize_scale, per_primitive) = collect_aabb(&gltf, blob);
    let raw_min = raw_aabb.0;
    let raw_max = raw_aabb.1;
    let extent = [
        raw_max[0] - raw_min[0],
        raw_max[1] - raw_min[1],
        raw_max[2] - raw_min[2],
    ];
    let nmin = [
        (raw_min[0] - center[0]) * normalize_scale,
        (raw_min[1] - center[1]) * normalize_scale,
        (raw_min[2] - center[2]) * normalize_scale,
    ];
    let nmax = [
        (raw_max[0] - center[0]) * normalize_scale,
        (raw_max[1] - center[1]) * normalize_scale,
        (raw_max[2] - center[2]) * normalize_scale,
    ];
    let nextent = [nmax[0] - nmin[0], nmax[1] - nmin[1], nmax[2] - nmin[2]];

    println!("\n=== Per-primitive AABBs (pre-normalisation, model-local space) ===");
    for (name, lo, hi) in &per_primitive {
        let ext = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
        println!("  {name:30}  min={lo:?}  max={hi:?}  ext={ext:?}");
    }

    println!("\n=== Global AABB (raw glTF space) ===");
    println!("  raw_aabb.min     = {raw_min:?}");
    println!("  raw_aabb.max     = {raw_max:?}");
    println!("  center           = {center:?}");
    println!("  extent           = {extent:?}");
    println!(
        "  max_extent       = {}",
        extent.iter().copied().fold(0.0f32, f32::max)
    );
    println!("  normalize_scale  = {normalize_scale}");

    println!("\n=== Normalized AABB (post T(-center)*S(normalize_scale)) ===");
    println!("  normalized.min   = {nmin:?}");
    println!("  normalized.max   = {nmax:?}");
    println!("  normalized.ext   = {nextent:?}");
    println!(
        "  aspect y/x       = {:.3}",
        nextent[1] / nextent[0].max(1e-6)
    );
    println!(
        "  aspect y/z       = {:.3}",
        nextent[1] / nextent[2].max(1e-6)
    );

    // ---- Skin summary (仮説 #1, #2, #3) ----
    println!("\n=== Skin summary (仮説 #1, #2) ===");
    println!("  total skins: {}", gltf.document.skins().count());
    let mut all_skin_joint_indices: Vec<Vec<usize>> = Vec::new();
    for (skin_idx, skin) in gltf.document.skins().enumerate() {
        let joints: Vec<usize> = skin.joints().map(|n| n.index()).collect();
        let ibm_count = skin
            .reader(|_| blob)
            .read_inverse_bind_matrices()
            .map(|ibm| ibm.count())
            .unwrap_or(0);
        let skeleton_root = skin.skeleton().map(|n| n.index());
        let mismatch = if ibm_count != joints.len() {
            " ★ MISMATCH"
        } else {
            ""
        };
        println!(
            "  skin[{skin_idx}]: joints={} ibm={} skeleton_root={:?}{mismatch}",
            joints.len(),
            ibm_count,
            skeleton_root
        );
        all_skin_joint_indices.push(joints);
    }

    // ---- Primitive -> skin mapping (仮説 #3) ----
    println!("\n=== Primitive -> skin mapping (仮説 #3) ===");
    // glTF 2.0 puts the skin reference on the NODE, not the
    // primitive. We walk every node, find ones that own a
    // mesh, and record the skin the node points to. The mesh
    // index is then keyed to that skin for every primitive
    // in the mesh.
    let mut mesh_to_skin: std::collections::HashMap<usize, Option<usize>> =
        std::collections::HashMap::new();
    for node in gltf.document.nodes() {
        if let Some(mesh) = node.mesh() {
            let skin_idx = node.skin().map(|s| s.index());
            mesh_to_skin
                .entry(mesh.index())
                .and_modify(|existing| {
                    if existing.is_none() && skin_idx.is_some() {
                        *existing = skin_idx;
                    }
                })
                .or_insert(skin_idx);
        }
    }
    println!("  mesh → skin via node:");
    for (&mesh_idx, skin_idx) in &mesh_to_skin {
        println!("    mesh[{mesh_idx}]: skin={skin_idx:?}");
    }
    let mut skin_ref_counts: std::collections::HashMap<Option<usize>, usize> =
        std::collections::HashMap::new();
    let mut nonzero_joint_count = 0usize;
    for (mesh_idx, mesh) in gltf.document.meshes().enumerate() {
        let mesh_skin = mesh_to_skin.get(&mesh_idx).copied().flatten();
        for (prim_idx, primitive) in mesh.primitives().enumerate() {
            let reader = primitive.reader(|_| blob);
            let joints: Option<Vec<[u16; 4]>> =
                reader.read_joints(0).map(|js| js.into_u16().collect());
            let max_joint_idx = joints
                .as_ref()
                .and_then(|v| v.iter().flat_map(|j| j.iter()).max().copied());
            let nonzero = max_joint_idx.map(|m| m > 0).unwrap_or(false);
            if nonzero {
                nonzero_joint_count += 1;
            }
            *skin_ref_counts.entry(mesh_skin).or_insert(0) += 1;
            println!(
                "  mesh[{mesh_idx}].prim[{prim_idx}]: node_skin={:?}  max_joint_idx={:?}  joints_nonzero={nonzero}",
                mesh_skin, max_joint_idx
            );
        }
    }
    println!("  skin ref distribution: {skin_ref_counts:?}");
    println!("  primitives with non-zero JOINTS_0: {nonzero_joint_count}");

    // ---- Humanoid bone -> joint index (仮説 #5) ----
    let humanoid = extract_humanoid_bones(&json);
    println!("\n=== Humanoid bone -> node (仮説 #5) ===");
    println!("  total humanoid bones declared: {}", humanoid.len());
    if !all_skin_joint_indices.is_empty() {
        let primary_skin: std::collections::HashSet<usize> =
            all_skin_joint_indices[0].iter().copied().collect();
        let mut in_skin = 0usize;
        let mut not_in_skin: Vec<String> = Vec::new();
        for (bone_name, node_idx) in &humanoid {
            if primary_skin.contains(node_idx) {
                in_skin += 1;
            } else {
                not_in_skin.push(bone_name.clone());
            }
        }
        not_in_skin.sort();
        println!("  in_skin[0]: {in_skin}/{}", humanoid.len());
        println!("  not_in_skin: {not_in_skin:?}");
    }
    for (bone_name, node_idx) in &humanoid {
        let in_skin = all_skin_joint_indices
            .first()
            .map(|s| s.contains(node_idx))
            .unwrap_or(false);
        let joint = all_skin_joint_indices
            .first()
            .and_then(|s| s.iter().position(|n| n == node_idx));
        let marker = if in_skin { " " } else { "★" };
        println!("  {marker} {bone_name:24} node={node_idx:4}  joint={joint:?}");
    }

    // ---- IBM alignment spot check (仮説 #1) ----
    println!("\n=== IBM alignment spot check (仮説 #1) ===");
    for (skin_idx, skin) in gltf.document.skins().enumerate() {
        let joint_nodes: Vec<_> = skin.joints().collect();
        let ibm_iter = skin.reader(|_| blob).read_inverse_bind_matrices();
        if let Some(ibm) = ibm_iter {
            let ibms: Vec<_> = ibm.collect();
            for (i, ((joint_node, ibm), joint_idx)) in
                joint_nodes.iter().zip(&ibms).zip(0..).take(8).enumerate()
            {
                let trans = [ibm[0][3], ibm[1][3], ibm[2][3]];
                println!(
                    "  skin[{skin_idx}].ibm[{i:3}] (joints[{i:3}]=node {}): translation={trans:?}",
                    joint_node.index()
                );
                let _ = joint_idx;
            }
            if ibms.len() > 8 {
                println!("  ... {} more ibm entries ...", ibms.len() - 8);
            }
        }
    }

    // ---- Projected world extents (existing diagnostic) ----
    let view_h = 2.6_f32;
    for &ms in &[0.5_f32, 1.0, 1.5, 2.0, 2.2, 3.0] {
        let world_h = nextent[1] * ms;
        let world_w = nextent[0] * ms;
        let fits_y = world_h <= view_h;
        let fits_x_at_1_1 = world_w <= view_h;
        println!(
            "  model_scale={ms:.2}  ->  world Y={world_h:.2} (viewport half_h={:.2}, fits={fits_y})  world X={world_w:.2} (1:1 fits={fits_x_at_1_1})",
            view_h * 0.5
        );
    }
}

type RawAabb = ([f32; 3], [f32; 3]);

fn collect_aabb(
    gltf: &gltf::Gltf,
    blob: Option<&[u8]>,
) -> (RawAabb, [f32; 3], f32, Vec<(String, [f32; 3], [f32; 3])>) {
    let mut all_positions: Vec<[f32; 3]> = Vec::new();
    let mut per_primitive: Vec<(String, [f32; 3], [f32; 3])> = Vec::new();
    for (mesh_idx, mesh) in gltf.document.meshes().enumerate() {
        for (prim_idx, primitive) in mesh.primitives().enumerate() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = primitive.reader(|_| blob);
            let Some(positions) = reader.read_positions() else {
                continue;
            };
            let positions: Vec<[f32; 3]> = positions.collect();
            if positions.is_empty() {
                continue;
            }
            let mut bb_min = positions[0];
            let mut bb_max = positions[0];
            for p in &positions {
                for i in 0..3 {
                    bb_min[i] = bb_min[i].min(p[i]);
                    bb_max[i] = bb_max[i].max(p[i]);
                }
            }
            per_primitive.push((format!("mesh[{mesh_idx}].prim[{prim_idx}]"), bb_min, bb_max));
            all_positions.extend(positions);
        }
    }
    let mut bb_min = all_positions[0];
    let mut bb_max = all_positions[0];
    for p in &all_positions {
        for i in 0..3 {
            bb_min[i] = bb_min[i].min(p[i]);
            bb_max[i] = bb_max[i].max(p[i]);
        }
    }
    let extent = [
        bb_max[0] - bb_min[0],
        bb_max[1] - bb_min[1],
        bb_max[2] - bb_min[2],
    ];
    let max_extent = extent.iter().copied().fold(0.0f32, f32::max);
    const TARGET_MODEL_SIZE: f32 = 1.5;
    let center = [
        (bb_min[0] + bb_max[0]) * 0.5,
        (bb_min[1] + bb_max[1]) * 0.5,
        (bb_min[2] + bb_max[2]) * 0.5,
    ];
    let scale = if max_extent > 0.0001 {
        TARGET_MODEL_SIZE / max_extent
    } else {
        1.0
    };
    ((bb_min, bb_max), center, scale, per_primitive)
}

/// Walk the JSON tree and pull out the humanoid bone list.
/// Returns `(bone_name, node_index)` for every entry under
/// `extensions.VRMC_vrm.humanoid.humanBones.<bone>`.
fn extract_humanoid_bones(json: &[u8]) -> Vec<(String, usize)> {
    let parsed: serde_json::Value = match serde_json::from_slice(json) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("failed to parse json: {e}");
            return Vec::new();
        }
    };
    let Some(ext) = parsed.get("extensions") else {
        return Vec::new();
    };
    let Some(vrm) = ext.get("VRMC_vrm") else {
        return Vec::new();
    };
    let Some(humanoid) = vrm.get("humanoid") else {
        return Vec::new();
    };
    let Some(bones) = humanoid.get("humanBones").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let mut out: Vec<(String, usize)> = Vec::new();
    for (raw_name, value) in bones {
        let Some(node_idx) = value
            .get("node")
            .and_then(|n| n.as_u64())
            .map(|n| n as usize)
        else {
            continue;
        };
        out.push((raw_name.clone(), node_idx));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn extract_first_glb(vrm_bytes: &[u8]) -> Option<Vec<u8>> {
    if vrm_bytes.len() < 4 || vrm_bytes[..4] != *b"PK\x03\x04" {
        if vrm_bytes.starts_with(b"glTF") {
            return Some(vrm_bytes.to_vec());
        }
        return None;
    }
    let cursor = std::io::Cursor::new(vrm_bytes);
    let mut archive = zip::ZipArchive::new(cursor).ok()?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).ok()?;
        if entry.name().to_ascii_lowercase().ends_with(".glb") {
            let mut out = Vec::new();
            use std::io::Read;
            entry.read_to_end(&mut out).ok()?;
            return Some(out);
        }
    }
    None
}
