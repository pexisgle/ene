//! Diagnostic: print the diagnostic values that
//! `apps/ene-desktop-v2` would log on the first frame.
//! Lets us sanity-check the loader's AABB / `normalize_scale` /
//! center / merged-skeleton joint count and the resulting
//! model matrix without spinning up a winit window.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

fn main() {
    let path: PathBuf = std::env::args()
        .nth(1).map_or_else(|| PathBuf::from("../assets/characters/Alicia/AliciaSolid.vrm"), PathBuf::from);
    println!("Reading {path:?}");

    let vrm_bytes = std::fs::read(&path).expect("read vrm file");
    let glb_bytes = extract_first_glb(&vrm_bytes).expect("extract .glb from .vrm");
    let mut cursor = 12usize;
    let mut json: Option<Vec<u8>> = None;
    let mut bin: Option<Vec<u8>> = None;
    while cursor + 8 <= glb_bytes.len() {
        let chunk_len = u32::from_le_bytes(
            glb_bytes[cursor..cursor + 4]
                .try_into()
                .expect("4-byte GLB chunk length slice"),
        ) as usize;
        let chunk_type = &glb_bytes[cursor + 4..cursor + 8];
        let payload_start = cursor + 8;
        let payload_end = payload_start + chunk_len;
        if payload_end > glb_bytes.len() {
            break;
        }
        let payload = &glb_bytes[payload_start..payload_end];
        match chunk_type {
            b"JSON" => json = Some(payload.to_vec()),
            b"BIN\0" => bin = Some(payload.to_vec()),
            _ => {}
        }
        let padded = (chunk_len + 3) & !3;
        cursor = payload_start + padded;
    }
    let json = json.expect("missing JSON chunk");
    let bin = bin.unwrap_or_default();
    let gltf = gltf::Gltf::from_slice(&json).expect("parse glTF JSON");
    let blob = if bin.is_empty() {
        None
    } else {
        Some(bin.as_slice())
    };

    // AABB / center / normalize_scale.
    let mut all_positions: Vec<[f32; 3]> = Vec::new();
    for mesh in gltf.document.meshes() {
        for primitive in mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = primitive.reader(|_| blob);
            if let Some(positions) = reader.read_positions() {
                all_positions.extend(positions);
            }
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
    let center = [
        (bb_min[0] + bb_max[0]) * 0.5,
        (bb_min[1] + bb_max[1]) * 0.5,
        (bb_min[2] + bb_max[2]) * 0.5,
    ];
    const TARGET_MODEL_SIZE: f32 = 1.5;
    let normalize_scale = if max_extent > 0.0001 {
        TARGET_MODEL_SIZE / max_extent
    } else {
        1.0
    };

    println!("\n=== Loader values (PR4.18) ===");
    println!("raw_aabb.min        = {bb_min:?}");
    println!("raw_aabb.max        = {bb_max:?}");
    println!("center              = {center:?}");
    println!("max_extent          = {max_extent}");
    println!("normalize_scale     = {normalize_scale}");
    println!(
        "normalized extent   = {:?}",
        [
            extent[0] * normalize_scale,
            extent[1] * normalize_scale,
            extent[2] * normalize_scale,
        ]
    );

    // Merged skeleton joint count (PR4.19).
    let mut all_joint_nodes: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for skin in gltf.document.skins() {
        for joint in skin.joints() {
            all_joint_nodes.insert(joint.index());
        }
    }
    println!("merged_skel.joints  = {}", all_joint_nodes.len());
    let total_skin_joints: usize = gltf.document.skins().map(|s| s.joints().count()).sum();
    println!("sum(per-skin joints) = {total_skin_joints}");
    println!(
        "(merge saved {} unique-vs-total dedup entries)",
        total_skin_joints as i64 - all_joint_nodes.len() as i64
    );

    // Auto-fit scale: 2.6 * 0.9 / 1.5 = 1.56.
    let viewport_h = 2.6_f32;
    let margin = 0.9_f32;
    let normalized_max_extent = max_extent * normalize_scale;
    let auto_fit_scale = viewport_h * margin / normalized_max_extent;
    println!("\n=== Camera + auto-fit (PR4.2) ===");
    println!("viewport_h          = {viewport_h}");
    println!("margin              = {margin}");
    println!("normalized_max_ext  = {normalized_max_extent}");
    println!("auto_fit_scale      = {auto_fit_scale}");

    // Model matrix (the runtime sends this to the shader).
    let character_position = [0.0_f32, 0.0, 0.0];
    let model_matrix = glam::Mat4::from_translation(character_position.into())
        * glam::Mat4::from_scale(glam::Vec3::splat(auto_fit_scale))
        * glam::Mat4::from_scale(glam::Vec3::splat(normalize_scale))
        * glam::Mat4::from_translation(glam::Vec3::from(center) * -1.0);

    println!("\n=== Model matrix (PR4.19) ===");
    println!("character_position  = {character_position:?}");
    println!("model_matrix        =");
    println!(
        "  [{:>9.4} {:>9.4} {:>9.4} {:>9.4}]",
        model_matrix.x_axis.x, model_matrix.y_axis.x, model_matrix.z_axis.x, model_matrix.w_axis.x
    );
    println!(
        "  [{:>9.4} {:>9.4} {:>9.4} {:>9.4}]",
        model_matrix.x_axis.y, model_matrix.y_axis.y, model_matrix.z_axis.y, model_matrix.w_axis.y
    );
    println!(
        "  [{:>9.4} {:>9.4} {:>9.4} {:>9.4}]",
        model_matrix.x_axis.z, model_matrix.y_axis.z, model_matrix.z_axis.z, model_matrix.w_axis.z
    );
    println!(
        "  [{:>9.4} {:>9.4} {:>9.4} {:>9.4}]",
        model_matrix.x_axis.w, model_matrix.y_axis.w, model_matrix.z_axis.w, model_matrix.w_axis.w
    );

    // Diagonal scale extraction.
    let scale_x = model_matrix.z_axis.x.mul_add(model_matrix.z_axis.x, model_matrix.y_axis.x.mul_add(model_matrix.y_axis.x, model_matrix.x_axis.x.powi(2)))
    .sqrt();
    let scale_y = model_matrix.z_axis.y.mul_add(model_matrix.z_axis.y, model_matrix.y_axis.y.mul_add(model_matrix.y_axis.y, model_matrix.x_axis.y.powi(2)))
    .sqrt();
    let scale_z = model_matrix.z_axis.z.mul_add(model_matrix.z_axis.z, model_matrix.y_axis.z.mul_add(model_matrix.y_axis.z, model_matrix.x_axis.z.powi(2)))
    .sqrt();
    println!("\ndiagonal scale (X, Y, Z) = ({scale_x:.4}, {scale_y:.4}, {scale_z:.4})");
    println!(
        "expected              = ({:.4}, {:.4}, {:.4})  (= auto_fit × normalize)",
        auto_fit_scale * normalize_scale,
        auto_fit_scale * normalize_scale,
        auto_fit_scale * normalize_scale,
    );

    // World Y range of (0, 0, 0) and (0, 1.65, 0) under the model matrix.
    let feet_raw = glam::Vec3::new(0.0, 0.0, 0.0);
    let head_raw = glam::Vec3::new(0.0, 1.65, 0.0);
    let feet_world = model_matrix.transform_point3(feet_raw);
    let head_world = model_matrix.transform_point3(head_raw);
    println!("\n=== World Y range (after model_matrix) ===");
    println!("feet (raw (0,0,0))     -> world y = {:.4}", feet_world.y);
    println!("head (raw (0,1.65,0))  -> world y = {:.4}", head_world.y);
    println!("viewport Y range      = [-1.3, 1.3]");
    println!(
        "model fits?           = {}",
        feet_world.y >= -1.3 && head_world.y <= 1.3
    );
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
