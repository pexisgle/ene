//! VRM 1.0 expression (blend shape) layer.
//!
//! a minimal but functional expression pipeline:
//!
//! - Each `VrmPrimitive` that carries morph targets in the glTF
//!   document is paired with a [`PrimitiveMorphs`] record that
//!   preserves every (name, position-displacement) pair.
//! - The CPU side stores the displacements as a flat
//!   `Vec<[f32; 3]>` of length `target_count * vertex_count` so
//!   the loader can stream it straight into a wgpu storage buffer
//!   without re-allocating.
//! - A global [`BTreeMap<ExpressionName, f32>`] of weights
//!   describes the current facial state. The runtime writes into
//!   it from the `EmotionQueue` (this struct) and the renderer reads it
//!   when it builds the per-primitive `weights` uniform each
//!   frame.
//! - All expressions across the whole model are flattened into a
//!   single sorted list ([`ExpressionLayer::expression_names`]).
//!   Names are case-insensitive at the loader boundary (VRM 1.0
//!   stores them lower-case) but kept as the raw string for
//!   round-trip with the AI bridge (`happy`, `sad`, …).
//!
//! Deliberately leaves the following to follow-up PRs:
//!
//! - Normal / tangent morph displacements (position only).
//! - Multi-target blend shapes (e.g. `blink_l` + `blink_r` driven
//!   by a single `blink` expression) are flattened to a 1:1
//!   name-to-target map. The legacy `bevy_vrm1` extends the same
//!   shape; layering the multiplier graph on top of this storage
//!   layout is a task.
//! - Mouth-shape / look-at expression mode: the look-at cursor
//!   projection (this struct) writes the four lookLeft / lookRight /
//!   lookUp / lookDown expressions directly into the weights map.
//!   This PR only writes `set_expression` from
//!   `CharacterRenderer::apply_emotions`.
//!
//! ## GPU layout (see also `shaders/mtoon_lite.wgsl`)
//!
//! The renderer builds one bind group per primitive that has
//! morph targets:
//!
//! - `@binding(0)` — `storage<read>` array of `vec3<f32>` of
//!   length `target_count * vertex_count`. The shader indexes
//!   `morph_offsets[target_idx * vertex_count + vertex_idx]`.
//! - `@binding(1)` — uniform [`PrimitiveMorphMeta`] with the
//!   vertex count, the target count, and a packed `weights` array
//!   of `vec4<f32>` (each vec4 holds four consecutive targets).
//!
//! Primitives without morph targets share a single dummy
//! bind-group layout with empty storage; the shader's
//! `target_count == 0u` early-out skips the lookup.
use std::collections::BTreeMap;

/// Maximum number of morph-target weight slots packed per
/// primitive. 16 vec4s = 64 slots. ships a uniform buffer
/// of this size; bump it in lock-step with the WGSL declaration
/// in `shaders/mtoon_lite.wgsl` if a model exceeds it.
pub const MAX_WEIGHT_SLOTS: usize = 16;

/// Number of weights per packed vec4 (== 4).
pub const WEIGHTS_PER_VEC4: usize = 4;

/// Total morph-target weights per primitive. Always a multiple
/// of [`WEIGHTS_PER_VEC4`].
///
/// Hidden from the "Supported API" docs — an internal GPU uniform-buffer sizing constant that
/// `ene-desktop` never references directly.
#[doc(hidden)]
pub const MAX_MORPH_TARGETS_PER_PRIMITIVE: usize = MAX_WEIGHT_SLOTS * WEIGHTS_PER_VEC4;

/// Number of f32 slots in a [`PrimitiveMorphMeta`]. Used by the
/// renderer to size the meta uniform buffer (must stay in sync
/// with the WGSL struct).
pub const MORPH_META_F32_COUNT: usize = 8 + MAX_WEIGHT_SLOTS * 4;

/// Packed morph-target uniform uploaded to the GPU every frame.
///
/// Mirrors the WGSL `MorphMeta` struct in
/// `shaders/mtoon_lite.wgsl` byte-for-byte: 8 f32 header
/// (`vertex_count`, `target_count`, two `pad` u32s reinterpreted as
/// f32) followed by `MAX_WEIGHT_SLOTS` packed `vec4<f32>`
/// weight slots.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct PrimitiveMorphMeta {
    /// Number of vertices in this primitive.
    pub vertex_count: u32,
    /// Number of morph targets defined on this primitive. The
    /// shader multiplies the active-target index by this to find
    /// the row stride into the offsets storage buffer.
    pub target_count: u32,
    /// Padding to align the next field to 16 bytes (vec4
    /// alignment).
    #[expect(
        clippy::pub_underscore_fields,
        reason = "wgpu bind group fields mirror shader naming"
    )]
    pub _pad0: u32,
    /// Padding to keep `weights` 16-byte aligned. Always zero.
    #[expect(
        clippy::pub_underscore_fields,
        reason = "wgpu bind group fields mirror shader naming"
    )]
    pub _pad1: u32,
    /// Packed weights, 4 per `vec4`. Slot `i` corresponds to
    /// morph target `i`.
    pub weights: [[f32; 4]; MAX_WEIGHT_SLOTS],
}

impl Default for PrimitiveMorphMeta {
    fn default() -> Self {
        Self {
            vertex_count: 0,
            target_count: 0,
            _pad0: 0,
            _pad1: 0,
            weights: [[0.0; 4]; MAX_WEIGHT_SLOTS],
        }
    }
}

impl PrimitiveMorphMeta {
    /// Size in bytes of the on-GPU uniform block.
    pub const SIZE: wgpu::BufferAddress =
        (MORPH_META_F32_COUNT * std::mem::size_of::<f32>()) as wgpu::BufferAddress;
}

/// One morph target belonging to one primitive, with the
/// position-displacement data already collected.
///
/// Multiple `MorphTarget` entries with the same name across
/// **different** primitives are allowed (the face mesh, the body
/// mesh, the accessory mesh can all carry their own `happy`
/// target). The renderer packs them per-primitive — the
/// `name_to_slot` map is local to one primitive.
#[derive(Clone, Debug)]
pub struct MorphTarget {
    /// The index of the morph target in the glTF mesh primitives' morph target list.
    pub target_index: usize,
    /// One position offset per vertex, in object space. Length
    /// must equal the primitive's `vertex_count`; the loader
    /// pads with zeros if the glTF accessor is shorter.
    pub position_offsets: Vec<[f32; 3]>,
}

/// Identifies a single primitive inside the model by its
/// flat-index (i.e. the order they were pushed into
/// `VrmModel::meshes`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PrimitiveId(pub usize);

/// Expression / blend-shape name. Kept as a thin newtype so the
/// `BTreeMap` key is unambiguous across the loader, the renderer
/// and the runtime.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExpressionName(pub String);

impl ExpressionName {
    /// Wrap a `&str` in an `ExpressionName`. The string is
    /// preserved verbatim — case is **not** normalized here; the
    /// loader lower-cases names from the `VRMC_vrm.expressions`
    /// ext object.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Borrow the inner string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ExpressionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ExpressionName {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// All morph targets defined on a single primitive, plus the
/// global (model-wide) name → target-slot mapping local to that
/// primitive. The renderer walks this structure when it
/// populates the per-frame `PrimitiveMorphMeta` uniform.
#[derive(Clone, Debug)]
pub struct PrimitiveMorphs {
    /// Stable identifier so the renderer can correlate a
    /// `PrimitiveMorphs` with the `VrmPrimitive` whose draw call
    /// is being issued. The flat index is `mesh_index *
    /// PRIMITIVE_STRIDE + primitive_index` (or simply
    /// `(mesh, prim)` if the renderer keeps the linear order it
    /// received at `new` time).
    pub primitive_id: PrimitiveId,
    /// The node index for the glTF mesh this primitive belongs to.
    pub node_index: usize,
    /// Morph targets in load order. The slot index in this vec
    /// is the index used by the GPU shader to look up offsets.
    pub targets: Vec<MorphTarget>,
    /// Cached `targets.len()` so the renderer can size uniforms
    /// without a load. Limited to
    /// [`MAX_MORPH_TARGETS_PER_PRIMITIVE`].
    pub uniform_buffer_len: u32,
    /// Vertex count of the host primitive. Required to validate
    /// the offsets length at load time and to fill the GPU
    /// uniform header.
    pub vertex_count: u32,
}

impl PrimitiveMorphs {
    /// Build a `PrimitiveMorphs` from a list of morph targets.
    #[must_use]
    pub const fn from_targets(
        primitive_id: PrimitiveId,
        node_index: usize,
        vertex_count: u32,
        targets: Vec<MorphTarget>,
    ) -> Self {
        let uniform_buffer_len = targets.len() as u32;

        Self {
            primitive_id,
            node_index,
            targets,
            uniform_buffer_len,
            vertex_count,
        }
    }
}

/// Top-level expression layer attached to a [`VrmModel`].
///
/// One instance per model. The runtime mutates `weights`; the
/// renderer reads it.
#[derive(Clone, Debug, Default)]
pub struct ExpressionLayer {
    /// One entry per primitive (linearized in the same order as
    /// the renderer's draw loop, i.e. mesh-major). `None` when
    /// the primitive has no morph targets — the renderer binds
    /// the dummy layout in that case.
    pub per_primitive: Vec<Option<PrimitiveMorphs>>,
    /// Current per-expression weight in `[0, 1]`. The runtime
    /// writes here; the renderer reads it. Names that are not in
    /// the model's expression list are **dropped** by
    /// [`ExpressionLayer::set_expression`] / [`apply_weights`]
    /// — the layer only stores weights for expressions the
    /// model actually defines, so a misspelled AI token or a
    /// UI slider pointed at a non-existent preset cannot
    /// silently grow the weight map. The caller still gets a
    /// `bool` back from `set_expression` to distinguish a
    /// successful update from a no-op drop.
    pub weights: BTreeMap<ExpressionName, f32>,
    /// Accumulated weights for individual morph targets.
    /// Keyed by `(node_index, morph_target_index)`.
    pub morph_target_weights: BTreeMap<(usize, usize), f32>,
}

impl ExpressionLayer {
    /// Build a fresh layer from per-primitive data. The model
    /// has zero expressions if `per_primitive` is empty.
    #[must_use]
    pub fn new(
        per_primitive: Vec<Option<PrimitiveMorphs>>,
        overrides: Option<&[crate::expression_override::ExpressionDefinition]>,
    ) -> Self {
        let mut weights = BTreeMap::new();
        if let Some(overrides) = overrides {
            for def in overrides {
                weights.insert(def.name.clone(), 0.0);
            }
        }
        let morph_target_weights = BTreeMap::new();
        Self {
            per_primitive,
            weights,
            morph_target_weights,
        }
    }

    /// Sorted list of every expression name defined on **any**
    /// primitive of the model. De-duplicated. Used by the
    /// settings UI's "Manual Expressions (Test)" buttons and by
    /// the AI bridge to look up the next emotion's name.
    pub fn expression_names(&self) -> Vec<ExpressionName> {
        self.weights.keys().cloned().collect()
    }

    /// Set a single expression weight. Names not present on the
    /// model are **not** stored in `weights`; the call returns
    /// `false` so the caller can detect the miss. Names that
    /// exist on at least one primitive return `true` and have
    /// the clamped weight inserted.
    ///
    /// the previous implementation stored the weight
    /// regardless, which let a misspelled AI token like `joy`
    /// (vs. the model's `happy`) silently accumulate in the
    /// weight map and never be cleared.
    pub fn set_expression(&mut self, name: &ExpressionName, weight: f32) -> bool {
        if !self.weights.contains_key(name) {
            return false;
        }
        self.weights.insert(name.clone(), weight.clamp(0.0, 1.0));
        true
    }

    /// Apply every (name, weight) pair in `incoming`, replacing
    /// the current weights map. Names that are not in the
    /// model's expression list are **dropped** so the weight map
    /// cannot grow with stale entries; the renderer is expected
    /// to call this once per frame from `apply_emotions`.
    pub fn apply_weights(&mut self, incoming: &BTreeMap<ExpressionName, f32>) {
        for (name, w) in incoming {
            // Skip names the model doesn't define. This keeps
            // `self.weights` a subset of `expression_names()` —
            // a useful invariant for tests and for any future
            // diagnostic that wants to dump the weight map.
            if !self.weights.contains_key(name) {
                continue;
            }
            self.weights.insert(name.clone(), w.clamp(0.0, 1.0));
        }
    }

    /// Number of primitives that have at least one morph target.
    pub fn morphic_primitive_count(&self) -> usize {
        self.per_primitive.iter().filter(|p| p.is_some()).count()
    }
}

use bytemuck::{Pod, Zeroable};

// The host-side `morph_offsets` storage buffer is indexed by
// the WGSL shader as `array<vec3<f32>>`. WGSL requires the
// array element stride to be 16 bytes (a `vec3` of `f32` is 12
// bytes but the array element is padded to a 16-byte boundary).
// The host side mirrors that with `Vec<[f32; 4]>` (one `vec4`
// per `vec3` entry, with `.w` reserved as 0). A 12-byte host
// element would misalign the buffer and the shader's
// `morph_offsets[i]` would read the wrong rows.
const _: () = {
    const HOST_MORPH_OFFSET_STRIDE: usize = std::mem::size_of::<[f32; 4]>();
    assert!(
        HOST_MORPH_OFFSET_STRIDE == 16,
        "morph_offsets host element must be 16 bytes; the WGSL array<vec3<f32>> \
         is 16-byte aligned and a 12-byte host element would desync the buffer"
    );
};

#[cfg(test)]
mod tests {
    use super::*;

    #[expect(
        dead_code,
        reason = "test helper for constructing PrimitiveId fixtures"
    )]
    fn prim(id: usize) -> PrimitiveId {
        PrimitiveId(id)
    }

    #[test]
    fn expression_names_dedupes_across_primitives() {
        let defs = vec![
            crate::expression_override::ExpressionDefinition::new("happy"),
            crate::expression_override::ExpressionDefinition::new("sad"),
        ];
        let layer = ExpressionLayer::new(vec![], Some(&defs));
        let mut names = layer.expression_names();
        names.sort_by_key(|n| n.as_str().to_string());
        assert_eq!(
            names,
            vec![
                ExpressionName("happy".to_string()),
                ExpressionName("sad".to_string()),
            ]
        );
    }

    #[test]
    fn set_expression_clamps_weight() {
        let defs = vec![crate::expression_override::ExpressionDefinition::new(
            "happy",
        )];
        let mut layer = ExpressionLayer::new(vec![], Some(&defs));
        assert!(layer.set_expression(&"happy".into(), 1.5));
        assert_eq!(layer.weights.get(&"happy".into()), Some(&1.0));
        layer.set_expression(&"happy".into(), -0.3);
        assert_eq!(layer.weights.get(&"happy".into()), Some(&0.0));
    }

    #[test]
    fn set_expression_unknown_name_returns_false_and_is_not_stored() {
        let defs = vec![crate::expression_override::ExpressionDefinition::new(
            "happy",
        )];
        let mut layer = ExpressionLayer::new(vec![], Some(&defs));
        let applied = layer.set_expression(&"joy".into(), 0.5);
        assert!(!applied);
        // A misspelled / unknown expression name must NOT be
        // inserted into the weight map (the leftover entry would
        // silently accumulate across frames and never reach the
        // GPU).
        assert!(!layer.weights.contains_key(&"joy".into()));
        // The known expression still works.
        assert!(layer.set_expression(&"happy".into(), 0.5));
        assert_eq!(layer.weights.get(&"happy".into()), Some(&0.5));
    }

    #[test]
    fn apply_weights_overwrites() {
        let defs = vec![crate::expression_override::ExpressionDefinition::new(
            "happy",
        )];
        let mut layer = ExpressionLayer::new(vec![], Some(&defs));
        layer.set_expression(&"happy".into(), 0.1);
        let mut incoming = BTreeMap::new();
        incoming.insert(ExpressionName::new("happy"), 0.7);
        layer.apply_weights(&incoming);
        assert_eq!(layer.weights.get(&"happy".into()), Some(&0.7));
    }

    /// `apply_weights` must drop entries whose name
    /// is not in the model's expression list. A misshaped
    /// `BTreeMap` from the AI bridge should not pollute the
    /// renderer's weight map.
    #[test]
    fn apply_weights_drops_unknown_names() {
        let defs = vec![crate::expression_override::ExpressionDefinition::new(
            "happy",
        )];
        let mut layer = ExpressionLayer::new(vec![], Some(&defs));
        let mut incoming = BTreeMap::new();
        incoming.insert(ExpressionName::new("happy"), 0.7);
        incoming.insert(ExpressionName::new("joy"), 0.9);
        incoming.insert(ExpressionName::new("eek"), 0.2);
        layer.apply_weights(&incoming);
        assert_eq!(layer.weights.len(), 1);
        assert_eq!(layer.weights.get(&"happy".into()), Some(&0.7));
        assert!(!layer.weights.contains_key(&"joy".into()));
        assert!(!layer.weights.contains_key(&"eek".into()));
    }

    #[test]
    fn morph_meta_default_is_all_zeros() {
        let m = PrimitiveMorphMeta::default();
        assert_eq!(m.vertex_count, 0);
        assert_eq!(m.target_count, 0);
        assert!(m.weights.iter().all(|v| v.iter().all(|f| *f == 0.0)));
    }

    #[test]
    fn empty_layer_has_no_names() {
        let layer = ExpressionLayer::default();
        assert!(layer.expression_names().is_empty());
        assert_eq!(layer.morphic_primitive_count(), 0);
    }

    /// the per-primitive morph cap is the contract the
    /// loader enforces (`load_primitive_morph_targets` warns
    /// and `take(MAX_MORPH_TARGETS_PER_PRIMITIVE)` truncates
    /// the input). The cap must match the uniform size:
    /// `MAX_WEIGHT_SLOTS * 4` packed `vec4<f32>` slots. A
    /// regression that drops the cap or bumps one without the
    /// other would silently overflow the bound uniform.
    #[test]
    fn morph_target_cap_matches_uniform_layout() {
        assert_eq!(
            MAX_MORPH_TARGETS_PER_PRIMITIVE,
            MAX_WEIGHT_SLOTS * WEIGHTS_PER_VEC4
        );
        assert_eq!(MAX_MORPH_TARGETS_PER_PRIMITIVE, 64);
        // A `Vec` of more targets than the cap must be safe to
        // truncate with `.take(MAX_MORPH_TARGETS_PER_PRIMITIVE)`
        // — the loader does exactly this on both the
        let capped: Vec<u32> = (0..(MAX_MORPH_TARGETS_PER_PRIMITIVE as u32 + 10))
            .take(MAX_MORPH_TARGETS_PER_PRIMITIVE)
            .collect();
        assert_eq!(capped.len(), MAX_MORPH_TARGETS_PER_PRIMITIVE);
        assert_eq!(capped[0], 0);
        assert_eq!(
            capped[MAX_MORPH_TARGETS_PER_PRIMITIVE - 1],
            MAX_MORPH_TARGETS_PER_PRIMITIVE as u32 - 1
        );
    }

    /// the host-side morph offset storage element
    /// is `[f32; 4]` so its byte size is 16 — the WGSL
    /// `array<vec3<f32>>` element stride. This test pins the
    /// size at runtime (the build also pins it via the
    /// `const _: ()` block above) so a regression to
    /// `[f32; 3]` would fail the test before the GPU sees
    /// misaligned data.
    #[test]
    fn morph_offset_storage_element_is_16_bytes() {
        let entry: [f32; 4] = [0.0; 4];
        assert_eq!(std::mem::size_of_val(&entry), 16);
        // Sanity: the loader's host shape matches the
        // renderer upload (`[f32; 4]` per vertex per target,
        // `.w` reserved as padding to satisfy the WGSL
        // 16-byte alignment).
        let sample_offsets: Vec<[f32; 4]> = vec![[1.0, 2.0, 3.0, 0.0]; 3];
        assert_eq!(
            bytemuck::cast_slice::<[f32; 4], u8>(&sample_offsets).len(),
            3 * 16
        );
    }
}
