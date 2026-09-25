use std::path::Path;

use serde_json::{Map, Value, json};

const FIXTURE_BONES: [&str; 15] = [
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

#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    #[error("fixture bone `{0}` is not part of the generated humanoid")]
    UnknownBone(String),
    #[error("fixture document could not be encoded: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("fixture is too large for a GLB container")]
    TooLarge,
    #[error("fixture image could not be encoded: {0}")]
    Image(#[from] image::ImageError),
    #[error("fixture file could not be written: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionFixture {
    pub bone: &'static str,
    pub yaw_degrees: f32,
    pub duration_secs: f32,
}

fn generated_vrm_glb() -> Result<Vec<u8>, FixtureError> {
    let mut encoded_png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(1, 1, image::Rgba([80, 160, 240, 255]))
        .write_to(&mut encoded_png, image::ImageFormat::Png)?;
    let encoded_png = encoded_png.into_inner();

    let head_mesh_node = bone_index("head")?;
    let nodes = FIXTURE_BONES
        .iter()
        .enumerate()
        .map(|(index, bone)| bone_node(bone, index, *bone == "head"))
        .collect::<Vec<_>>();
    let human_bones = human_bones();
    let expression = || {
        json!({
            "morphTargetBinds": [{ "node": head_mesh_node, "index": 0, "weight": 1.0 }]
        })
    };
    let document = json!({
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
                    "joints": [{ "node": head_mesh_node, "stiffness": 1.0, "dragForce": 0.4 }]
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

    let positions_and_target = [
        -0.5_f32, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.05, 0.0,
        0.0,
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

fn generated_vrma_glb(motion: MotionFixture) -> Result<Vec<u8>, FixtureError> {
    let animated = bone_index(motion.bone)?;
    let nodes = FIXTURE_BONES
        .iter()
        .enumerate()
        .map(|(index, bone)| bone_node(bone, index, false))
        .collect::<Vec<_>>();
    let extension = json!({
        "specVersion": "1.0",
        "humanoid": { "humanBones": human_bones() }
    });
    let document = json!({
        "asset": { "version": "2.0" },
        "extensionsUsed": ["VRMC_vrm_animation"],
        "extensionsRequired": ["VRMC_vrm_animation"],
        "extensions": { "VRMC_vrm_animation": extension },
        "nodes": nodes,
        "animations": [{
            "channels": [{ "sampler": 0, "target": { "node": animated, "path": "rotation" } }],
            "samplers": [{ "input": 0, "interpolation": "LINEAR", "output": 1 }]
        }],
        "buffers": [{ "byteLength": 40 }],
        "bufferViews": [
            { "buffer": 0, "byteOffset": 0, "byteLength": 8 },
            { "buffer": 0, "byteOffset": 8, "byteLength": 32 }
        ],
        "accessors": [
            {
                "bufferView": 0,
                "componentType": 5126,
                "count": 2,
                "type": "SCALAR",
                "min": [0.0],
                "max": [motion.duration_secs]
            },
            { "bufferView": 1, "componentType": 5126, "count": 2, "type": "VEC4" }
        ]
    });

    let half = motion.yaw_degrees.to_radians() / 2.0;
    let binary = [0.0_f32, motion.duration_secs]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .chain(
            [0.0_f32, 0.0, 0.0, 1.0, 0.0, half.sin(), 0.0, half.cos()]
                .iter()
                .flat_map(|value| value.to_le_bytes()),
        )
        .collect::<Vec<_>>();
    glb(&document, &binary)
}

pub fn write_generated_vrm(path: &Path) -> Result<(), FixtureError> {
    create_parent(path)?;
    std::fs::write(path, generated_vrm_glb()?)?;
    Ok(())
}

pub fn write_generated_vrma(path: &Path, motion: MotionFixture) -> Result<(), FixtureError> {
    create_parent(path)?;
    std::fs::write(path, generated_vrma_glb(motion)?)?;
    Ok(())
}

fn create_parent(path: &Path) -> Result<(), FixtureError> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => std::fs::create_dir_all(parent)?,
        _ => {}
    }
    Ok(())
}

fn bone_index(bone: &str) -> Result<usize, FixtureError> {
    FIXTURE_BONES
        .iter()
        .position(|name| *name == bone)
        .ok_or_else(|| FixtureError::UnknownBone(bone.to_string()))
}

fn bone_children(index: usize) -> Vec<usize> {
    match FIXTURE_BONES.get(index) {
        Some(&"hips") => vec![1, 3, 6],
        Some(&"spine") => vec![2, 9, 12],
        Some(&"leftUpperLeg") => vec![4],
        Some(&"leftLowerLeg") => vec![5],
        Some(&"rightUpperLeg") => vec![7],
        Some(&"rightLowerLeg") => vec![8],
        Some(&"leftUpperArm") => vec![10],
        Some(&"leftLowerArm") => vec![11],
        Some(&"rightUpperArm") => vec![13],
        Some(&"rightLowerArm") => vec![14],
        _ => Vec::new(),
    }
}

fn bone_node(bone: &str, index: usize, mesh: bool) -> Value {
    let mut node = Map::new();
    node.insert(String::from("name"), json!(bone));
    node.insert(
        String::from("translation"),
        json!([0.0, index as f32 * 0.05, 0.0]),
    );
    let children = bone_children(index);
    if !children.is_empty() {
        node.insert(String::from("children"), json!(children));
    }
    if mesh {
        node.insert(String::from("mesh"), json!(0));
    }
    Value::Object(node)
}

fn human_bones() -> Map<String, Value> {
    FIXTURE_BONES
        .iter()
        .enumerate()
        .map(|(index, bone)| (bone.to_string(), json!({ "node": index })))
        .collect()
}

fn glb(document: &Value, binary: &[u8]) -> Result<Vec<u8>, FixtureError> {
    let mut json = serde_json::to_vec(document)?;
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut bin = binary.to_vec();
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let total = u32::try_from(total).map_err(|_| FixtureError::TooLarge)?;
    let mut out = Vec::with_capacity(usize::try_from(total).map_err(|_| FixtureError::TooLarge)?);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2_u32.to_le_bytes());
    out.extend_from_slice(&total.to_le_bytes());
    out.extend_from_slice(
        &u32::try_from(json.len())
            .map_err(|_| FixtureError::TooLarge)?
            .to_le_bytes(),
    );
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(
        &u32::try_from(bin.len())
            .map_err(|_| FixtureError::TooLarge)?
            .to_le_bytes(),
    );
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    Ok(out)
}
