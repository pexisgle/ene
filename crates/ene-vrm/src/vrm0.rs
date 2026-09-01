//! Legacy VRM 0.x conversion layer.
//!
//! A VRM 0.x document stores its data under a root extension named
//! `VRM` instead of `VRMC_vrm`, with several wire-format quirks:
//! permission fields are spelled `UssageName`, spring stiffness is
//! `stiffiness`, blend-shape weights are percentages, and humanoid
//! bones live in an array rather than a keyed map. The
//! [VRM-0.x specification](https://github.com/vrm-c/vrm-specification/blob/master/specification/VRM-0.x/README.md)
//! is authoritative for the JSON layout.
//!
//! Every converter here fills the **same** runtime structs the 1.0
//! parsers produce so the renderer keeps a single drawing path and
//! 1.0 behaviour is untouched.

use glam::{Quat, Vec3};
use serde_json::Value;

use crate::expression_override::{ExpressionDefinition, MorphTargetBind};
use crate::humanoid::canonicalize_bone_name;
use crate::humanoid::{BoneRestTransform, HumanoidBoneEntry, HumanoidBoneRegistry};
use crate::look_at::{LookAtProperties, LookAtRangeMap, LookAtRangeMapSet, LookAtType};
use crate::model::Skeleton;
use crate::mtoon::MToonMaterial;
use crate::spring_bone::{
    DEFAULT_DRAG_FORCE, DEFAULT_GRAVITY_DIR, DEFAULT_GRAVITY_POWER, DEFAULT_HIT_RADIUS,
    DEFAULT_STIFFNESS, SpringBoneChain, SpringBoneCollider, SpringBoneColliderGroup,
    SpringBoneJoint, SpringBoneProperties, SpringBoneShape,
};

/// Mirror of the legacy `meta` block plus usage permissions. In 0.x they live
/// together (`meta.allowedUserName`, `meta.violentUssageName`, ...) while 1.0
/// splits permissions into its own section; both dialects land in this struct
/// shape so consumers read one form.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Vrm0Meta {
    /// Display title (`title`; falls back to `name`).
    pub(crate) title: String,
    /// Author / publisher name.
    pub(crate) author: String,
    /// Contact information for the author.
    pub(crate) contact_information: Option<String>,
    /// Reference URL(s) for the original work.
    pub(crate) reference: Option<String>,
    /// Who may use the model (`allowedUserName`).
    pub(crate) allowed_user_name: AllowedUserName,
    /// Whether violent-expression usage is permitted (`violentUssageName`, sic).
    pub(crate) violent_usage: UsagePermission,
    /// Whether sexual-expression usage is permitted (`sexualUssageName`, sic).
    pub(crate) sexual_usage: UsagePermission,
    /// Whether commercial usage is permitted (`commercialUssageName`, sic).
    pub(crate) commercial_usage: UsagePermission,
    /// License URL when the licence is expressed as a link (`licenseUrl`).
    pub(crate) license_url: Option<String>,
    /// Free-form additional licence text (`otherLicenseUrl`).
    pub(crate) other_license_url: String,
}

/// Who may use the model, from `meta.allowedUserName`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum AllowedUserName {
    /// Only the author may use the model (`OnlyAuthor`).
    OnlyAuthor,
    /// Anyone may use the model; what shipped exporters emit when the
    /// field is present but unparsable.
    #[default]
    Everyone,
}

impl AllowedUserName {
    fn parse(raw: &str) -> Self {
        match raw {
            "OnlyAuthor" => Self::OnlyAuthor,
            _ => Self::default(),
        }
    }
}

/// Per-category usage permission from the 0.x meta block. Shipped exporters
/// emit `Disallow` / `Allow` / `Limit` / `AllowIsNotNecessary`; anything that
/// does not deny outright counts as permitted so hosts can still prompt for
/// approval, mirroring how the 1.0 parser keeps unparsable enum strings
/// loadable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum UsagePermission {
    /// The category is forbidden (`Disallow`).
    Denied,
    /// Unrestricted or approval-gated use; the published-model default.
    #[default]
    Permitted,
}

impl UsagePermission {
    fn parse(raw: &str) -> Self {
        match raw {
            "Disallow" => Self::Denied,
            _ => Self::default(),
        }
    }
}

fn root_vrm(gltf: &gltf::Gltf) -> Option<&Value> {
    gltf.document.extensions()?.get("VRM")
}

fn as_object(value: Option<&Value>) -> Option<&serde_json::Map<String, Value>> {
    value?.as_object()
}

fn parse_vec3(arr: &[Value]) -> [f32; 3] {
    let mut out = [0.0_f32; 3];
    for (i, v) in arr.iter().take(3).enumerate() {
        out[i] = v.as_f64().unwrap_or(0.0) as f32;
    }
    out
}

fn vec3_field(obj: &serde_json::Map<String, Value>, key: &str) -> Option<[f32; 3]> {
    Some(parse_vec3(obj.get(key)?.as_array()?))
}
/// Parse the legacy `meta` block. Called only on detected VRM 0.x documents;
/// missing or malformed fields degrade to their defaults rather than failing
/// the whole load, mirroring how the 1.0 parser treats optional meta data.
#[doc(hidden)]
pub(crate) fn parse_meta(gltf: &gltf::Gltf) -> Option<Vrm0Meta> {
    let meta = as_object(root_vrm(gltf)?.get("meta"))?;
    let mut parsed = Vrm0Meta {
        title: meta
            .get("title")
            .or_else(|| meta.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        author: meta
            .get("author")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        contact_information: meta
            .get("contactName")
            .and_then(Value::as_str)
            .map(str::to_owned),
        reference: meta
            .get("reference")
            .and_then(Value::as_str)
            .map(str::to_owned),
        allowed_user_name: meta
            .get("allowedUserName")
            .and_then(Value::as_str)
            .map_or_else(AllowedUserName::default, AllowedUserName::parse),
        violent_usage: meta
            .get("violentUssageName")
            .and_then(Value::as_str)
            .map_or_else(UsagePermission::default, UsagePermission::parse),
        sexual_usage: meta
            .get("sexualUssageName")
            .and_then(Value::as_str)
            .map_or_else(UsagePermission::default, UsagePermission::parse),
        commercial_usage: meta
            .get("commercialUssageName")
            .and_then(Value::as_str)
            .map_or_else(UsagePermission::default, UsagePermission::parse),
        license_url: meta
            .get("licenseUrl")
            .and_then(Value::as_str)
            .map(str::to_owned),
        other_license_url: meta
            .get("otherLicenseUrl")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
    };
    if parsed.title.is_empty() {
        // The 0.x spec makes `title` optional while GUI labels need a non-empty value.
        "unnamed".clone_into(&mut parsed.title);
    }
    Some(parsed)
}

/// Convert the legacy `humanoid.humanBones` array (1.0 uses an object keyed by
/// bone name) into the shared registry. Joint lookup and rest transforms come
/// from the merged skeleton built earlier in the load path, matching the 1.0
/// loader behaviour.
#[doc(hidden)]
pub(crate) fn convert_humanoid(gltf: &gltf::Gltf, skeleton: &Skeleton) -> HumanoidBoneRegistry {
    let mut registry = HumanoidBoneRegistry::new();
    let bones = as_object(root_vrm(gltf).and_then(|v| v.get("humanoid")))
        .and_then(|h| h.get("humanBones").and_then(Value::as_array));
    let Some(bones) = bones else {
        return registry;
    };

    for entry in bones {
        let Some(raw_name) = entry.get("bone").and_then(Value::as_str) else {
            continue;
        };
        let Some(canonical) = canonicalize_bone_name(raw_name) else {
            tracing::warn!(
                raw_name,
                "VRM humanoid humanBones has unknown bone name; dropped"
            );
            continue;
        };
        let Some(node_idx) = entry
            .get("node")
            .and_then(Value::as_u64)
            .and_then(|idx| usize::try_from(idx).ok())
        else {
            tracing::warn!(
                raw_name,
                "VRM humanoid humanBones entry is missing a valid node; dropped",
            );
            continue;
        };
        let joint = skeleton.joint_to_node.iter().position(|&n| n == node_idx);
        let rest =
            gltf.document
                .nodes()
                .nth(node_idx)
                .map_or(BoneRestTransform::default(), |node| {
                    let (translation, rotation, _scale) = node.transform().decomposed();
                    BoneRestTransform {
                        translation: Vec3::from(translation),
                        rotation: Quat::from_array(rotation),
                    }
                });
        if !registry.insert(
            canonical.clone(),
            HumanoidBoneEntry {
                node: node_idx,
                joint,
                rest,
            },
        ) {
            tracing::warn!(%canonical, "VRM humanoid duplicate bone entry; first kept");
        }
    }

    tracing::info!(bones = registry.len(), "VRM 0.x humanoid converted");
    registry
}
/// Convert legacy `blendShapeMaster.blendShapeGroups` into the 1.0
/// `ExpressionDefinition` list. Each group becomes one named expression;
/// binds keep the shared `(mesh, index, weight)` shape, with percent weights
/// rescaled into the runtime 0-1 range.
#[doc(hidden)]
pub(crate) fn convert_blendshapes(gltf: &gltf::Gltf) -> Vec<ExpressionDefinition> {
    let mut defs = Vec::new();
    let Some(groups) = as_object(root_vrm(gltf).and_then(|v| v.get("blendShapeMaster")))
        .and_then(|m| m.get("blendShapeGroups").and_then(Value::as_array))
    else {
        return defs;
    };

    for group in groups {
        let name = match group.get("presetName").and_then(Value::as_str) {
            Some(preset) if !preset.is_empty() => preset.to_owned(),
            _ => match group.get("name").and_then(Value::as_str) {
                Some(name) => format!("custom_{name}"),
                None => continue,
            },
        };
        // Legacy groups carry no 1.0-style override block; leaving the
        // settings at their defaults keeps procedural gating consistent
        // with expressions parsed from a 1.0 file.
        let mut def = ExpressionDefinition::new(name.as_str());
        for bind in group
            .get("binds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(mesh) = bind
                .get("mesh")
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok())
            else {
                continue;
            };
            let Some(index) = bind
                .get("index")
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok())
            else {
                continue;
            };
            let weight = (bind.get("weight").and_then(Value::as_f64).unwrap_or(100.0) / 100.0)
                .clamp(0.0, 1.0) as f32;
            // The legacy wire format binds by mesh, but the runtime struct and
            // the renderer consume node indices; resolve every node that uses
            // the mesh so shared meshes stay reachable.
            for (node_idx, node) in gltf.document.nodes().enumerate() {
                if node.mesh().map(|m| m.index()) != Some(mesh) {
                    continue;
                }
                def.morph_target_binds.push(MorphTargetBind {
                    node: node_idx,
                    index,
                    weight,
                });
            }
            if def.morph_target_binds.is_empty() {
                tracing::warn!(
                    mesh,
                    "VRM blendShape bind references a mesh no node uses; dropped",
                );
            }
        }
        defs.push(def);
    }

    tracing::info!(groups = defs.len(), "VRM 0.x blendShapeGroups converted");
    defs
}

/// Convert the legacy `firstPerson` look-at fields. The 1.0 block uses nested
/// `rangeMap*` objects; 0.x stores four flat curve objects with an
/// `xRange`/`yRange` degree pair and targets a named bone (`lookAtBoneName`)
/// rather than built-in expression slots.
#[doc(hidden)]
pub(crate) fn convert_look_at(gltf: &gltf::Gltf) -> Option<LookAtProperties> {
    let fp = as_object(root_vrm(gltf)?.get("firstPerson"))?;
    let mut props = LookAtProperties {
        offset_from_head_bone: vec3_field(fp, "firstPersonBoneOffset").unwrap_or([0.0, 0.06, 0.0]),
        range_map: LookAtRangeMapSet {
            horizontal_inner: parse_legacy_range(fp, "lookAtHorizontalInner").unwrap_or_default(),
            horizontal_outer: parse_legacy_range(fp, "lookAtHorizontalOuter").unwrap_or_default(),
            vertical_down: parse_legacy_range(fp, "lookAtVerticalDown").unwrap_or_default(),
            vertical_up: parse_legacy_range(fp, "lookAtVerticalUp").unwrap_or_default(),
        },
        look_at_type: LookAtType::default(),
    };
    if fp.get("lookAtTypeName").and_then(Value::as_str) == Some("BlendShape") {
        props.look_at_type = LookAtType::Expression;
    }
    Some(props)
}

fn parse_legacy_range(fp: &serde_json::Map<String, Value>, key: &str) -> Option<LookAtRangeMap> {
    let curve = as_object(fp.get(key))?;
    let x_range = curve.get("xRange").and_then(Value::as_f64)?;
    let y_range = curve.get("yRange").and_then(Value::as_f64)?;
    // The 0.x curve is sampled at (xRange, yRange); exporters emit degrees.
    Some(LookAtRangeMap {
        input_max_value: x_range.max(0.0) as f32,
        output_scale: y_range.max(0.0) as f32,
    })
}
/// Convert legacy `secondaryAnimation` into the 1.0 spring-bone properties.
/// 0.x has no standalone collider list - colliders live inside
/// `boneGroups.colliderGroups` with node, offset and radius inline. The legacy
/// field spellings (`stiffiness`) are part of the wire format and are checked
/// before the corrected spelling.
#[doc(hidden)]
pub(crate) fn convert_spring_bones(gltf: &gltf::Gltf) -> Option<SpringBoneProperties> {
    let secondary = as_object(root_vrm(gltf)?.get("secondaryAnimation"))?;
    let mut colliders: Vec<SpringBoneCollider> = Vec::new();
    let mut collider_groups: Vec<SpringBoneColliderGroup> = Vec::new();
    let mut springs = Vec::new();

    for group in secondary
        .get("colliderGroups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = group.get("name").and_then(Value::as_str).map(str::to_owned);
        let mut indices = Vec::new();
        for collider in group
            .get("colliders")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(node) = collider
                .get("node")
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok())
            else {
                continue;
            };
            let radius = collider
                .get("radius")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            indices.push(colliders.len());
            colliders.push(SpringBoneCollider {
                node,
                shape: SpringBoneShape::Sphere {
                    offset: collider
                        .get("offset")
                        .and_then(Value::as_array)
                        .map_or([0.0; 3], |arr| parse_vec3(arr)),
                    radius: radius.max(0.0) as f32,
                },
            });
        }
        collider_groups.push(SpringBoneColliderGroup {
            name,
            collider_indices: indices,
        });
    }

    for group in secondary
        .get("boneGroups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let joints: Vec<SpringBoneJoint> = group
            .get("bones")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|node| {
                let node = usize::try_from(node.as_u64()?).ok()?;
                Some(SpringBoneJoint {
                    node,
                    hit_radius: group
                        .get("hitRadius")
                        .and_then(Value::as_f64)
                        .unwrap_or(f64::from(DEFAULT_HIT_RADIUS))
                        .max(0.0) as f32,
                    stiffness: group
                        .get("stiffiness")
                        .or_else(|| group.get("stiffness"))
                        .and_then(Value::as_f64)
                        .unwrap_or(f64::from(DEFAULT_STIFFNESS))
                        .max(0.0) as f32,
                    gravity_power: group
                        .get("gravityPower")
                        .and_then(Value::as_f64)
                        .unwrap_or(f64::from(DEFAULT_GRAVITY_POWER))
                        .max(0.0) as f32,
                    gravity_dir: group
                        .get("gravityDir")
                        .and_then(Value::as_array)
                        .map_or(DEFAULT_GRAVITY_DIR, |arr| parse_vec3(arr)),
                    drag_force: group
                        .get("dragForce")
                        .and_then(Value::as_f64)
                        .unwrap_or(f64::from(DEFAULT_DRAG_FORCE))
                        .clamp(0.0, 1.0) as f32,
                })
            })
            .collect();
        if joints.is_empty() {
            continue;
        }
        springs.push(SpringBoneChain {
            name: group.get("name").and_then(Value::as_str).map(str::to_owned),
            joints,
            collider_group_indices: group
                .get("colliderGroups")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|idx| usize::try_from(idx.as_u64()?).ok())
                .collect(),
            // A group without a center keeps world-space inertia.
            center: group
                .get("center")
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok()),
        });
    }

    Some(SpringBoneProperties {
        spec_version: "legacy-0.x".to_owned(),
        colliders,
        collider_groups,
        springs,
    })
}
/// Build MToon materials from the legacy per-material `VRM` extension. 1.0
/// carries its MToon parameters in `VRMC_materials_mtoon`; a legacy file stores
/// shader identity in `_shader` plus `renderQueue`, which map onto the same
/// runtime slots the 1.0 parser fills. Materials without the legacy block stay
/// `None` so the renderer keeps its PBR fallback.
#[doc(hidden)]
pub(crate) fn load_mtoon_materials_vrm0(gltf: &gltf::Gltf) -> Vec<Option<MToonMaterial>> {
    gltf.document
        .materials()
        .map(|material| {
            let obj = material.extensions()?.get("VRM")?.as_object()?;
            let mut mat = MToonMaterial::default();
            if let Some(queue) = obj.get("renderQueue").and_then(Value::as_i64) {
                // Legacy render queues are 3000-based (`Geometry` = 3000); the
                // runtime stores a -9..9 offset from that base.
                mat.render_queue_offset = (queue - 3000).clamp(-9, 9) as i32;
            }
            // Shader identity lives in the per-material legacy block
            // (exporters differ between "Shader" and "_shader" keys);
            // top-level material keys are not glTF extensions and are
            // invisible to the parser here.
            if let Some("VRM/MToon/TransparentZWrite") = obj
                .get("Shader")
                .or_else(|| obj.get("_shader"))
                .and_then(Value::as_str)
            {
                mat.transparent_with_z_write = true;
            }
            Some(mat)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{AllowedUserName, UsagePermission, as_object, parse_vec3, vec3_field};
    use serde_json::{Map, json};

    #[test]
    fn allowed_user_name_parses_only_author() {
        assert_eq!(
            AllowedUserName::parse("OnlyAuthor"),
            AllowedUserName::OnlyAuthor
        );
        assert_eq!(AllowedUserName::parse(""), AllowedUserName::Everyone);
        assert_eq!(
            AllowedUserName::parse("Everyone"),
            AllowedUserName::Everyone
        );
    }

    #[test]
    fn usage_permission_denies_only_disallow() {
        assert_eq!(UsagePermission::parse("Disallow"), UsagePermission::Denied);
        assert_eq!(UsagePermission::parse("Allow"), UsagePermission::Permitted);
        assert_eq!(UsagePermission::parse(""), UsagePermission::Permitted);
    }

    #[test]
    fn parse_vec3_pads_and_ignores_non_numbers() {
        let empty = parse_vec3(&[]);
        assert!(empty.iter().all(|v| v.abs() < f32::EPSILON));
        let parsed = parse_vec3(&[json!(1.0), json!(2), json!("x"), json!(9.0)]);
        assert!((parsed[0] - 1.0).abs() < f32::EPSILON);
        assert!((parsed[1] - 2.0).abs() < f32::EPSILON);
        assert!(parsed[2].abs() < f32::EPSILON);
    }

    #[test]
    fn as_object_and_vec3_field_round_trip() {
        assert!(as_object(None).is_none());
        assert!(as_object(Some(&json!(1))).is_none());
        let mut map = Map::new();
        map.insert("g".into(), json!([0.0, 1.0, 0.0]));
        let got = vec3_field(&map, "g").expect("g");
        assert!(got[0].abs() < f32::EPSILON);
        assert!((got[1] - 1.0).abs() < f32::EPSILON);
        assert!(got[2].abs() < f32::EPSILON);
        assert!(vec3_field(&map, "missing").is_none());
    }
}
