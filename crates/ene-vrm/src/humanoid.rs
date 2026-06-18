//! VRMC_vrm-1.0 humanoid bone registry.
//!
//! PR4.7: parses the `VRMC_vrm.humanoid.humanBones` extension
//! object, resolves each named bone to a glTF `Node` index,
//! captures the rest-pose local transform, and exposes the
//! result as a typed lookup table.
//!
//! ## Spec background
//!
//! The VRM 1.0 humanoid spec
//! ([`VRMC_vrm-1.0/humanoid.md`](https://github.com/vrm-c/vrm-specification/blob/master/specification/VRMC_vrm-1.0/humanoid.md))
//! declares a fixed set of 55 humanoid bone names. Each
//! model lists only the bones it actually uses; the loader
//! walks the `humanBones` map and turns every entry into a
//! [`HumanoidBoneEntry`] with the glTF `Node` index, an
//! optional `Skeleton` joint index (so consumers can find
//! the inverse-bind matrix in O(1) without re-walking the
//! skin), and the local rest transform
//! (translation + rotation quaternion).
//!
//! ## Conventions
//!
//! - **Bone names are lower-case canonical** to match the
//!   spec's JSON keys (e.g. `"hips"`, `"leftupperarm"`). The
//!   [`canonicalize_bone_name`] helper accepts common
//!   variants (`"Hips"`, `"LeftUpperArm"`,
//!   `"left_upper_arm"`, `"left-upper-arm"`) so hand-edited
//!   VRM files still load, but the registry key is always
//!   the spec form.
//! - **Unknown bone names are dropped with a warning.** A
//!   typo or a VRoid extension bone ends up as a `tracing::warn!`
//!   line and the entry is not added to the registry.
//! - **The registry is a `BTreeMap` so iteration order is
//!   stable** for diagnostics dumps and snapshot tests.
//!
//! ## Consumers
//!
//! This module is the **foundation** for the deferred
//! follow-up issues:
//!
//! - #11 (`VRMC_vrm.lookAt`) — uses `head()`, `left_eye()`,
//!   `right_eye()` and the rest rotation to apply the
//!   `calc_yaw_pitch` quaternion delta.
//! - #13 (`VRMC_springBone`) — `hips()` is the natural root
//!   of the main chain, `head()` is a head-bone input.
//! - #14 (`VRMC_vrm_animation`) — bone-name → node-index
//!   retarget for humanoid tracks.
//! - #15 (`VRMC_node_constraint`) — bone-name lookup for
//!   constraint sources / destinations.
//!
//! PR4.7 itself only **builds the registry**; none of the
//! per-frame consumers land in this commit.
use std::collections::BTreeMap;

use glam::{Quat, Vec3};

use crate::model::Skeleton;

/// The 55 humanoid bone names defined by the VRM 1.0 spec,
/// in lower-case canonical form. Order is the spec's
/// "head → spine → arms → fingers → legs" layout so the
/// registry iterates in a predictable order for diagnostics.
///
/// The set matches the bone list used by the legacy
/// `bevy_vrm1` `bone_marker_component!` macro: 9 trunk bones,
/// 8 arm bones, 24 finger bones (4 fingers × 3 segments on
/// each hand; the thumb has 3 segments — metacarpal +
/// proximal + distal — and the other fingers have 3
/// segments — proximal + intermediate + distal), and 8 leg
/// bones. See [`canonicalize_bone_name`] for the case /
/// separator variants the loader accepts.
pub const HUMANOID_BONE_NAMES: &[&str] = &[
    // Trunk + head (9)
    "hips",
    "spine",
    "chest",
    "upperchest",
    "neck",
    "head",
    "jaw",
    "lefteye",
    "righteye",
    // Arms (8)
    "leftshoulder",
    "leftupperarm",
    "leftlowerarm",
    "lefthand",
    "rightshoulder",
    "rightupperarm",
    "rightlowerarm",
    "righthand",
    // Thumbs (6)
    "leftthumbmetacarpal",
    "leftthumbproximal",
    "leftthumbdistal",
    "rightthumbmetacarpal",
    "rightthumbproximal",
    "rightthumbdistal",
    // Index fingers (6)
    "leftindexproximal",
    "leftindexintermediate",
    "leftindexdistal",
    "rightindexproximal",
    "rightindexintermediate",
    "rightindexdistal",
    // Middle fingers (6)
    "leftmiddleproximal",
    "leftmiddleintermediate",
    "leftmiddledistal",
    "rightmiddleproximal",
    "rightmiddleintermediate",
    "rightmiddledistal",
    // Ring fingers (6)
    "leftringproximal",
    "leftringintermediate",
    "leftringdistal",
    "rightringproximal",
    "rightringintermediate",
    "rightringdistal",
    // Little fingers (6)
    "leftlittleproximal",
    "leftlittleintermediate",
    "leftlittledistal",
    "rightlittleproximal",
    "rightlittleintermediate",
    "rightlittledistal",
    // Legs (8)
    "leftupperleg",
    "leftlowerleg",
    "leftfoot",
    "lefttoes",
    "rightupperleg",
    "rightlowerleg",
    "rightfoot",
    "righttoes",
];

/// Typed humanoid bone name. Wraps a `String` for use as a
/// `BTreeMap` key. The inner value is always the
/// spec-defined lower-case form (e.g. `"hips"`,
/// `"leftupperarm"`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VrmBone(pub String);

impl VrmBone {
    /// Borrow the inner canonical string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for VrmBone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for VrmBone {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for VrmBone {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Local rest-pose transform of a humanoid bone, captured at
/// load time. The translation / rotation are read straight
/// from the glTF `Node` transform (which is local to the
/// node's parent, not global). Per-bone (one entry per bone)
/// so consumers (LookAt, SpringBone, VRMA, NodeConstraint)
/// can read it without re-walking the glTF node hierarchy.
///
/// Scale is intentionally omitted: the loader's
/// per-vertex `normalize(center, scale)` is the canonical
/// scale, and per-bone scale is not consumed by any of the
/// deferred issues. (Add `scale: [f32; 3]` here if a future
/// consumer needs it; the loader change is one line.)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoneRestTransform {
    /// Local translation `[x, y, z]`.
    pub translation: Vec3,
    /// Local rotation as a quaternion.
    pub rotation: Quat,
}

impl Default for BoneRestTransform {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
        }
    }
}

/// One entry in the humanoid registry. `node` is the glTF
/// `Node` index (always set — a humanoid bone with no node
/// is malformed and the loader drops it). `joint` is the
/// optional index into `Skeleton::inverse_bind` so LookAt /
/// VRMA can find the joint matrix in O(1) without re-walking
/// the skin.
#[derive(Clone, Debug)]
pub struct HumanoidBoneEntry {
    /// glTF `Node` index of the bone.
    pub node: usize,
    /// Optional index into `Skeleton::inverse_bind`. `None`
    /// when the model has a humanoid bone outside the skin
    /// (rare but spec-legal — e.g. an `_experimental` bone
    /// that the exporter didn't register with the skin).
    pub joint: Option<usize>,
    /// Local rest-pose transform.
    pub rest: BoneRestTransform,
}

/// The humanoid bone registry. Stores one entry per
/// `humanBones.<name>` the loader found. Iteration is
/// deterministic (BTreeMap, sorted by canonical bone name).
#[derive(Clone, Debug, Default)]
pub struct HumanoidBoneRegistry {
    entries: BTreeMap<VrmBone, HumanoidBoneEntry>,
}

impl HumanoidBoneRegistry {
    /// Build an empty registry. Most code paths should use
    /// [`load_humanoid_bones`] instead.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert an entry. Returns `true` if the bone was new,
    /// `false` if the same name was already present (the
    /// existing entry is left untouched — last-write-wins
    /// would silently overwrite a real entry on a malformed
    /// model).
    pub fn insert(&mut self, bone: VrmBone, entry: HumanoidBoneEntry) -> bool {
        if self.entries.contains_key(&bone) {
            return false;
        }
        self.entries.insert(bone, entry);
        true
    }

    /// Look up an entry by its canonical (lower-case) bone
    /// name. Use [`Self::by_name`] to accept the spec's
    /// mixed-case form too.
    pub fn lookup(&self, bone: &VrmBone) -> Option<&HumanoidBoneEntry> {
        self.entries.get(bone)
    }

    /// Look up an entry by a possibly-mixed-case / mixed-
    /// separator bone name. `canonicalize_bone_name` does
    /// the heavy lifting; unknown names return `None`.
    pub fn by_name(&self, raw_name: &str) -> Option<&HumanoidBoneEntry> {
        let bone = canonicalize_bone_name(raw_name)?;
        self.entries.get(&bone)
    }

    /// Convenience accessor for the `head` bone.
    pub fn head(&self) -> Option<&HumanoidBoneEntry> {
        self.by_name("head")
    }

    /// Convenience accessor for the `hips` bone (the root
    /// of the humanoid chain).
    pub fn hips(&self) -> Option<&HumanoidBoneEntry> {
        self.by_name("hips")
    }

    /// Convenience accessor for the `chest` bone. Used as the
    /// body-center fallback when the model has no `head` bone
    /// (see `apps/ene-desktop-v2::character::body_center_world`).
    pub fn chest(&self) -> Option<&HumanoidBoneEntry> {
        self.by_name("chest")
    }

    /// Convenience accessor for the `jaw` bone.
    pub fn jaw(&self) -> Option<&HumanoidBoneEntry> {
        self.by_name("jaw")
    }

    /// Convenience accessor for the `leftEye` bone.
    pub fn left_eye(&self) -> Option<&HumanoidBoneEntry> {
        self.by_name("lefteye")
    }

    /// Convenience accessor for the `rightEye` bone.
    pub fn right_eye(&self) -> Option<&HumanoidBoneEntry> {
        self.by_name("righteye")
    }

    /// Iterate `(bone, entry)` in canonical (sorted) order.
    pub fn iter(&self) -> impl Iterator<Item = (&VrmBone, &HumanoidBoneEntry)> {
        self.entries.iter()
    }

    /// Sorted list of registered canonical bone names.
    pub fn names(&self) -> Vec<VrmBone> {
        self.entries.keys().cloned().collect()
    }

    /// Number of registered bones.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when no bones are registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Map a possibly-canonical bone name to the spec's
/// lower-case form. The loader sees a mix of styles in
/// the wild (lower-case per the spec, `PascalCase` from
/// hand-written models, `snake_case` and `kebab-case` from
/// tooling exports). We accept them all and return the
/// canonical form on success.
///
/// Returns `None` for unknown names — the loader logs a
/// warning and drops the entry. The match table is built
/// once (an `OnceLock<BTreeMap<&str, &str>>` would be the
/// next step; the current linear scan is fast enough for
/// 55 entries and keeps the module dependency-free).
pub fn canonicalize_bone_name(raw: &str) -> Option<VrmBone> {
    // First normalise: drop separators and lowercase. This
    // collapses "Hips" / "hips" / "HIPS" / "left_upper_arm" /
    // "left-upper-arm" / "LeftUpperArm" to "leftupperarm".
    let mut normalised = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch == '_' || ch == '-' || ch == ' ' {
            continue;
        }
        for lower in ch.to_lowercase() {
            normalised.push(lower);
        }
    }
    // Then look up the spec form. The match arm is the
    // canonical name; we only need to verify it is in the
    // `HUMANOID_BONE_NAMES` set.
    if HUMANOID_BONE_NAMES.contains(&normalised.as_str()) {
        Some(VrmBone(normalised))
    } else {
        None
    }
}

/// Parse `Document::extensions()["VRMC_vrm"]["humanoid"]["humanBones"]`
/// and build a [`HumanoidBoneRegistry`]. `skel` is the
/// already-loaded skeleton — the joint-index lookup uses
/// `Skeleton::joint_to_node`, which maps skin-joint index
/// to glTF node index.
///
/// Unknown bone names are dropped with a `tracing::warn!`
/// (a hand-edited model with a typo or a VRoid extension
/// bone should not panic the loader). Missing node
/// indices, or `Node::transform()` calls that fail, are
/// also warned and the entry is stored with a
/// default `BoneRestTransform`.
pub fn load_humanoid_bones(gltf: &gltf::Gltf, skel: &Skeleton) -> HumanoidBoneRegistry {
    let mut registry = HumanoidBoneRegistry::new();

    // 1. Locate the `humanBones` map. The path mirrors
    //    `resolve_expression_names` so the rest of the
    //    loader reads extension objects the same way.
    let Some(ext) = gltf.document.extensions() else {
        return registry;
    };
    let Some(vrm) = ext.get("VRMC_vrm") else {
        return registry;
    };
    let Some(humanoid) = vrm.get("humanoid") else {
        return registry;
    };
    let Some(bones) = humanoid.get("humanBones").and_then(|v| v.as_object()) else {
        return registry;
    };

    // 2. Walk every entry. Keys are bone names (lower-case
    //    per spec, but `canonicalize_bone_name` is
    //    permissive). Values are `{ "node": <usize> }`.
    for (raw_name, value) in bones {
        let Some(canonical) = canonicalize_bone_name(raw_name) else {
            tracing::warn!(
                "VRM humanoid.humanBones has unknown bone name '{}' (dropped)",
                raw_name
            );
            continue;
        };
        let Some(node_idx) = value
            .get("node")
            .and_then(|n| n.as_u64())
            .map(|n| n as usize)
        else {
            tracing::warn!(
                "VRM humanoid.humanBones['{}'] is missing a 'node' field (dropped)",
                raw_name
            );
            continue;
        };

        // 3. Resolve the joint index by walking the skin's
        //    joint list. None when the bone is outside the
        //    skin — legal but rare; we still keep the entry.
        let joint = skel.joint_to_node.iter().position(|&n| n == node_idx);

        // 4. Capture the local rest transform from the glTF
        //    node. A missing / out-of-range node is logged
        //    and we fall back to the identity transform.
        let rest = match gltf.document.nodes().nth(node_idx) {
            Some(node) => {
                // gltf 1.4's `Transform` exposes
                // `decomposed() -> ([f32;3], [f32;4], [f32;3])`
                // (translation, rotation quaternion, scale).
                let (translation, rotation, _scale) = node.transform().decomposed();
                BoneRestTransform {
                    translation: Vec3::from(translation),
                    rotation: Quat::from_array(rotation),
                }
            }
            None => {
                tracing::warn!(
                    "VRM humanoid.humanBones['{}'] references out-of-range node {} (rest transform = identity)",
                    raw_name,
                    node_idx
                );
                BoneRestTransform::default()
            }
        };

        let entry = HumanoidBoneEntry {
            node: node_idx,
            joint,
            rest,
        };
        if !registry.insert(canonical.clone(), entry) {
            tracing::warn!(
                "VRM humanoid.humanBones['{}'] is a duplicate (first entry kept)",
                canonical
            );
        }
    }

    if !registry.is_empty() {
        tracing::info!(
            "VRM humanoid: {} bone(s) registered (out of 55 spec-defined)",
            registry.len()
        );
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 55-bone set is the canonical contract. A
    /// regression (a bone dropped, a duplicate added) would
    /// silently break PR4.7's foundation.
    #[test]
    fn bone_name_set_has_55_unique_entries() {
        assert_eq!(HUMANOID_BONE_NAMES.len(), 55);
        // BTreeSet-style dedup check via a `Vec` of owned
        // `String`s. There is no `HashSet` in `no_std`-free
        // `std` we cannot use here; the linear scan is
        // O(N^2) = 3000 ops, fine for a unit test.
        let mut seen: Vec<&str> = Vec::with_capacity(HUMANOID_BONE_NAMES.len());
        for name in HUMANOID_BONE_NAMES.iter() {
            assert!(!seen.contains(name), "duplicate bone name {name}");
            assert_eq!(
                name.chars().all(|c| c.is_ascii_lowercase()),
                true,
                "bone name {name} is not lower-case ASCII"
            );
            assert!(
                !name.contains('_') && !name.contains('-'),
                "bone name {name} must not contain separators"
            );
            seen.push(name);
        }
    }

    /// The canonicalize helper must accept the spec's
    /// lower-case form plus the common variants that
    /// exporters and hand-edited files produce.
    #[test]
    fn canonicalize_bone_name_handles_known_variants() {
        // Lower-case is the spec form.
        assert_eq!(canonicalize_bone_name("hips").unwrap().0, "hips");
        assert_eq!(canonicalize_bone_name("head").unwrap().0, "head");
        assert_eq!(canonicalize_bone_name("lefteye").unwrap().0, "lefteye");
        // Upper-case.
        assert_eq!(canonicalize_bone_name("HIPS").unwrap().0, "hips");
        // Mixed case (VRoid, hand-edited files).
        assert_eq!(canonicalize_bone_name("Hips").unwrap().0, "hips");
        assert_eq!(
            canonicalize_bone_name("LeftUpperArm").unwrap().0,
            "leftupperarm"
        );
        assert_eq!(canonicalize_bone_name("RightEye").unwrap().0, "righteye");
        // snake_case (tooling exports).
        assert_eq!(
            canonicalize_bone_name("left_upper_arm").unwrap().0,
            "leftupperarm"
        );
        // kebab-case (some Unity exporters).
        assert_eq!(
            canonicalize_bone_name("left-upper-arm").unwrap().0,
            "leftupperarm"
        );
        // With surrounding spaces (rare but cheap to accept).
        assert_eq!(canonicalize_bone_name("  hips  ").unwrap().0, "hips");
    }

    /// A typo or VRoid extension bone must return `None`,
    /// not a silent registry entry.
    #[test]
    fn canonicalize_bone_name_returns_none_for_typo() {
        assert!(canonicalize_bone_name("hip").is_none());
        assert!(canonicalize_bone_name("hipz").is_none());
        // Separators are stripped, so the result `lefteye` is
        // a valid spec form — see the `handles_known_variants`
        // test. A truly unknown name with a separator (`tail`)
        // must still drop out.
        assert!(canonicalize_bone_name("").is_none());
        assert!(canonicalize_bone_name("tail").is_none());
        assert!(canonicalize_bone_name("some_random_bone").is_none());
    }

    /// The registry should accept an entry, reject a
    /// duplicate, and look it up by both the typed and
    /// raw-name paths.
    #[test]
    fn registry_lookup_by_canonical_name() {
        let mut reg = HumanoidBoneRegistry::new();
        let entry = HumanoidBoneEntry {
            node: 14,
            joint: Some(0),
            rest: BoneRestTransform {
                translation: Vec3::new(0.0, 1.5, 0.0),
                rotation: Quat::IDENTITY,
            },
        };
        assert!(reg.insert(VrmBone("hips".into()), entry.clone()));
        // Duplicate insert returns false; existing entry kept.
        assert!(!reg.insert(VrmBone("hips".into()), entry.clone()));
        // Typed lookup.
        let got = reg.lookup(&VrmBone("hips".into())).unwrap();
        assert_eq!(got.node, 14);
        assert_eq!(got.joint, Some(0));
        // Raw lookup with case / separator variants.
        assert!(reg.by_name("Hips").is_some());
        assert!(reg.by_name("hips").is_some());
        assert!(reg.by_name("HIPS").is_some());
        assert!(reg.by_name("not-a-bone").is_none());
    }

    /// The named-bone helpers (`head`, `hips`, `left_eye`,
    /// ...) must point at the right canonical entries. A
    /// regression that swapped `left_eye` and `right_eye`
    /// would silently mirror every model's gaze.
    #[test]
    fn registry_helper_accessors_return_correct_bones() {
        let mut reg = HumanoidBoneRegistry::new();
        for (name, node) in [
            ("hips", 1),
            ("head", 14),
            ("lefteye", 22),
            ("righteye", 23),
            ("jaw", 15),
        ] {
            reg.insert(
                VrmBone(name.into()),
                HumanoidBoneEntry {
                    node,
                    joint: None,
                    rest: BoneRestTransform::default(),
                },
            );
        }
        assert_eq!(reg.hips().unwrap().node, 1);
        assert_eq!(reg.head().unwrap().node, 14);
        assert_eq!(reg.jaw().unwrap().node, 15);
        assert_eq!(reg.left_eye().unwrap().node, 22);
        assert_eq!(reg.right_eye().unwrap().node, 23);
        // Iteration is sorted.
        let names: Vec<String> = reg.names().into_iter().map(|n| n.0).collect();
        assert_eq!(
            names,
            vec!["head", "hips", "jaw", "lefteye", "righteye"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    /// The registry should reflect what the loader found —
    /// a model with no `VRMC_vrm.humanoid` block gets an
    /// empty registry, not a panic.
    #[test]
    fn empty_registry_is_default_state() {
        let reg = HumanoidBoneRegistry::default();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.head().is_none());
        assert!(reg.hips().is_none());
        assert!(reg.left_eye().is_none());
        assert!(reg.right_eye().is_none());
    }

    /// `BoneRestTransform` defaults to identity (the glTF
    /// spec's "no transform applied" semantic). Consumers
    /// like LookAt use `rest.rotation * yaw_pitch_delta`
    /// and expect identity to be the safe baseline.
    #[test]
    fn bone_rest_transform_default_is_identity() {
        let rest = BoneRestTransform::default();
        assert_eq!(rest.translation, Vec3::ZERO);
        assert_eq!(rest.rotation, Quat::IDENTITY);
    }

    /// The end-to-end registry build path must accept a
    /// mixed-case / mixed-separator bone name and produce
    /// the canonical (lower-case, no separators) form
    /// regardless of how the upstream JSON file is spelled.
    /// This is the same path the loader's `load_humanoid_bones`
    /// walks; testing it directly avoids needing to construct
    /// a real `gltf::Gltf` (which requires a full glTF
    /// document).
    #[test]
    fn registry_built_from_canonicalize_then_insert() {
        // Simulate what `load_humanoid_bones` does for
        // each entry: canonicalise the bone name, then
        // insert the resolved entry.
        let mut reg = HumanoidBoneRegistry::new();
        for raw in ["Hips", "Head", "LeftUpperArm", "left_eye", "right_eye"] {
            let canonical = canonicalize_bone_name(raw).expect("known bone");
            reg.insert(
                canonical.clone(),
                HumanoidBoneEntry {
                    node: 0,
                    joint: None,
                    rest: BoneRestTransform::default(),
                },
            );
        }
        // All five entries registered under their canonical form.
        assert_eq!(reg.len(), 5);
        let names: Vec<String> = reg.names().into_iter().map(|n| n.0).collect();
        assert_eq!(
            names,
            vec!["head", "hips", "lefteye", "leftupperarm", "righteye"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }
}
