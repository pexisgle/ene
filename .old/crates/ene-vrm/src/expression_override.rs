//! VRM 1.0 expression override semantics.
//!
//! The `VRMC_vrm.expressions` block ships per-expression metadata
//! — `isBinary`, `overrideMouth`, `overrideBlink`,
//! `overrideLookAt` — that controls how non-procedural
//! expressions (e.g. `happy`, `sad`) interact with procedural
//! expressions (e.g. `blink`, `aa`, `lookUp`).
//!
//! This module ships:
//!
//! - [`ExpressionOverrideType`] — `None` | `Block` | `Blend`
//! - [`ExpressionOverrideSettings`] — per-expression triple
//!   `{ mouth, blink, look_at }` of override types
//! - [`ExpressionDefinition`] — `{ name, overrides, is_binary }`
//! - [`apply_overrides`] — the frame-level override evaluator
//!   (isBinary clamp + block / blend for every procedural target)
//! - [`load_expression_overrides`] — JSON walker that reads
//!   the `VRMC_vrm.expressions.{preset,custom}` tree alongside
//!   the existing `morphTargetBinds` walker in the loader.
//!
//! ## Override semantics (spec §Procedural override)
//!
//! | Value  | Effect                                                          |
//! |--------|-----------------------------------------------------------------|
//! | `none` | do nothing                                                      |
//! | `block`| target weight → 0 when source weight > 0                       |
//! | `blend`| target weight *= (1 − sum of source weights), clamped to [0, 1] |
//!
//! ## isBinary interaction (§Interaction between override and isBinary)
//!
//! - When an expression **with** `isBinary` overrides another, the
//!   **binary output value** (0 or 1 based on the ≥0.5 threshold)
//!   is used for the override computation.
//! - When an expression **with** `isBinary` is overridden, it MUST
//!   be completely suppressed (weight → 0) if the override effect
//!   is greater than 0.0.
//!
//! ## Procedural target sets
//!
//! | Override field      | Targets                                          |
//! |---------------------|--------------------------------------------------|
//! | `overrideMouth`     | `aa`, `ih`, `ou`, `ee`, `oh`                     |
//! | `overrideBlink`     | `blink`, `blinkLeft`, `blinkRight`               |
//! | `overrideLookAt`    | `lookUp`, `lookDown`, `lookLeft`, `lookRight`    |
//!
//! Same-kind override (e.g. `blink.overrideBlink = block`) is
//! treated as invalid and ignored per spec.
use std::collections::BTreeMap;

use crate::expression::ExpressionName;

/// Procedural mouth-shape target names (lip sync).
///
/// Hidden from the "Supported API" docs — used internally by [`is_procedural`] and
/// [`apply_overrides`]; `ene-desktop` never needs the raw name lists.
#[doc(hidden)]
pub const MOUTH_TARGET_NAMES: &[&str] = &["aa", "ih", "ou", "ee", "oh"];

#[doc(hidden)]
pub const BLINK_TARGET_NAMES: &[&str] = &["blink", "blinkLeft", "blinkRight"];

#[doc(hidden)]
pub const GAZE_TARGET_NAMES: &[&str] = &["lookUp", "lookDown", "lookLeft", "lookRight"];

#[doc(hidden)]
pub fn is_procedural(name: &str) -> bool {
    MOUTH_TARGET_NAMES.contains(&name)
        || BLINK_TARGET_NAMES.contains(&name)
        || GAZE_TARGET_NAMES.contains(&name)
}

/// How an expression overrides a procedural expression category.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ExpressionOverrideType {
    /// No override (spec default).
    #[default]
    None,
    /// Target weight → 0 when source weight > 0.
    Block,
    /// Target weight *= (1 − sum of source weights).
    Blend,
}

impl ExpressionOverrideType {
    /// Parse from a VRM JSON string (`"none"`, `"block"`,
    /// `"blend"`). Unknown strings silently fall back to `None`
    /// (the spec default) so a malformed file does not blow up
    /// the loader.
    pub fn from_json_str(s: &str) -> Self {
        match s {
            "block" => Self::Block,
            "blend" => Self::Blend,
            _ => Self::None,
        }
    }
}

/// Per-expression override settings, paired on load with every
/// preset / custom expression definition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ExpressionOverrideSettings {
    /// Override lip sync (`aa`, `ih`, `ou`, `ee`, `oh`).
    pub mouth: ExpressionOverrideType,
    /// Override blink (`blink`, `blinkLeft`, `blinkRight`).
    pub blink: ExpressionOverrideType,
    /// Override gaze (`lookUp`, `lookDown`, `lookLeft`, `lookRight`).
    pub look_at: ExpressionOverrideType,
}

#[derive(Clone, Debug)]
/// Maps a morph target index of a glTF node to an expression definition.
pub struct MorphTargetBind {
    /// The glTF node index that contains the mesh to morph.
    pub node: usize,
    /// The morph target index on the mesh primitives.
    pub index: usize,
    /// The weight scalar applied to the morph target when this expression is at 1.0.
    pub weight: f32,
}

/// A parsed definition from the `VRMC_vrm.expressions` glTF extension.
#[derive(Clone, Debug)]
pub struct ExpressionDefinition {
    /// Expression name (e.g. `happy`, `sad`).
    pub name: ExpressionName,
    pub overrides: ExpressionOverrideSettings,
    /// `weight > 0.5` -> 1.0, else 0.0.
    pub is_binary: bool,
    /// Morph targets driven by this expression.
    pub morph_target_binds: Vec<MorphTargetBind>,
}

impl ExpressionDefinition {
    /// Build a definition with spec-default overrides (`none`
    /// everywhere, `is_binary = false`). Callers that parsed
    /// a partial JSON block use this as the base and then
    /// override individual fields.
    pub fn new(name: impl Into<ExpressionName>) -> Self {
        Self {
            name: name.into(),
            overrides: ExpressionOverrideSettings::default(),
            is_binary: false,
            morph_target_binds: Vec::new(),
        }
    }
}

/// Evaluate the `isBinary` clamp and the
/// `overrideMouth/Blink/LookAt` block / blend semantics against
/// `weights`, using the per-expression definitions.
///
/// The function mutates `weights` in place. Call this every
/// frame **after** `set_expression` / `apply_weights` (i.e. after
/// the raw per-frame weights have been written) and **before**
/// the renderer reads the weight map to build the GPU uniform.
///
/// ## Algorithm
///
/// 1. **isBinary clamp**: for every definition with `is_binary`,
///    the corresponding weight in the map is rounded to 0 or 1
///    (≥0.5 → 1.0, <0.5 → 0.0).
///
/// 2. **override collection**: for every source expression `s`
///    whose (post-clamp) weight > 0, and for every procedural
///    target `t` that `s` overrides, accumulate the contribution.
///    `block` marks the target as blocked; `blend` adds the
///    source weight to the attenuation sum.
///    Same-kind override (e.g. `blink` → `blink`) is skipped.
///
/// 3. **application**: for each affected target `t`:
///    - If blocked → weight → 0.
///    - If blend-sum > 0 and `t` itself has `is_binary` →
///      weight → 0 (spec: "MUST be completely suppressed if the
///      effect received is greater than 0.0").
///    - Otherwise → weight *= (1 − blend-sum).
///
/// When a model ships no expression definitions (e.g. legacy
/// VRM 0.x with no `VRMC_vrm.expressions` block), the function
/// is a no-op — the weight map passes through unchanged.
pub fn apply_overrides(weights: &mut BTreeMap<ExpressionName, f32>, defs: &[ExpressionDefinition]) {
    if defs.is_empty() || weights.is_empty() {
        return;
    }

    for def in defs {
        if def.is_binary
            && let Some(w) = weights.get(&def.name)
        {
            let binary = if *w >= 0.5 { 1.0 } else { 0.0 };
            weights.insert(def.name.clone(), binary);
        }
    }

    // Keyed by target name: (blend_sum, is_blocked). Block wins
    // over blend.
    let mut factors: BTreeMap<ExpressionName, (f32, bool)> = BTreeMap::new();

    for def in defs {
        let effective = weights.get(&def.name).copied().unwrap_or(0.0);
        if effective <= 0.0 {
            continue;
        }

        let fields: [(ExpressionOverrideType, &[&str]); 3] = [
            (def.overrides.mouth, MOUTH_TARGET_NAMES),
            (def.overrides.blink, BLINK_TARGET_NAMES),
            (def.overrides.look_at, GAZE_TARGET_NAMES),
        ];

        for (override_type, target_set) in &fields {
            match override_type {
                ExpressionOverrideType::None => continue,
                ExpressionOverrideType::Block | ExpressionOverrideType::Blend => {}
            }
            for target in *target_set {
                // Same-kind override (e.g. blink → blink) is
                // invalid per spec — skip.
                if def.name.as_str() == *target {
                    continue;
                }
                let entry = factors
                    .entry(ExpressionName::new(*target))
                    .or_insert((0.0, false));
                match override_type {
                    ExpressionOverrideType::Block => {
                        entry.1 = true;
                    }
                    ExpressionOverrideType::Blend => {
                        entry.0 = (entry.0 + effective).min(1.0);
                    }
                    ExpressionOverrideType::None => {}
                }
            }
        }
    }

    for (target_name, (blend_sum, is_blocked)) in &factors {
        let original = weights.get(target_name).copied().unwrap_or(0.0);
        if original == 0.0 {
            continue;
        }

        let target_def = defs.iter().find(|d| d.name == *target_name);

        let new = if *is_blocked {
            0.0
        } else if *blend_sum > 0.0 {
            // Spec: "When an expression with isBinary is
            // overridden by other expressions, the expression
            // MUST be completely suppressed if the effect
            // received is greater than 0.0."
            if target_def.is_some_and(|d| d.is_binary) {
                0.0
            } else {
                original * (1.0 - blend_sum)
            }
        } else {
            original
        };

        if (new - original).abs() > f32::EPSILON {
            weights.insert(target_name.clone(), new);
        }
    }
}

/// Walk the `VRMC_vrm.expressions.{preset,custom}.<name>` tree
/// and collect the per-expression `{ isBinary, overrideMouth,
/// overrideBlink, overrideLookAt }` fields into a
/// `Vec<ExpressionDefinition>`.
///
/// Missing fields fall back to the spec defaults (`isBinary =
/// false`, all overrides = `"none"`) — so a malformed block
/// never panics. Unknown override strings are silently treated
/// as `"none"`.
///
/// Returns an empty Vec when the `VRMC_vrm.expressions` block is
/// missing (e.g. VRM 0.x models), so the override pass becomes a
/// no-op.
///
/// Called internally by [`crate::loader::load_vrm`]; hidden from the "Supported API" docs —
/// hosts get the result via [`VrmModel::expressions_meta`](crate::model::VrmModel).
#[doc(hidden)]
pub fn load_expression_overrides(gltf: &gltf::Gltf) -> Vec<ExpressionDefinition> {
    let mut defs = Vec::new();

    let Some(ext) = gltf.document.extensions() else {
        return defs;
    };
    let Some(vrm) = ext.get("VRMC_vrm") else {
        return defs;
    };
    let Some(expressions) = vrm.get("expressions") else {
        return defs;
    };
    let Some(expressions_obj) = expressions.as_object() else {
        return defs;
    };

    let mut parsed_count = 0usize;

    for (group_name, group) in expressions_obj {
        if group_name != "preset" && group_name != "custom" {
            continue;
        }
        let Some(exprs) = group.as_object() else {
            continue;
        };
        for (name, expr) in exprs {
            let mut def = ExpressionDefinition::new(name.as_str());

            def.is_binary = expr
                .get("isBinary")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);

            def.overrides.mouth = expr
                .get("overrideMouth")
                .and_then(|v| v.as_str())
                .map(ExpressionOverrideType::from_json_str)
                .unwrap_or_default();

            def.overrides.blink = expr
                .get("overrideBlink")
                .and_then(|v| v.as_str())
                .map(ExpressionOverrideType::from_json_str)
                .unwrap_or_default();

            def.overrides.look_at = expr
                .get("overrideLookAt")
                .and_then(|v| v.as_str())
                .map(ExpressionOverrideType::from_json_str)
                .unwrap_or_default();

            if let Some(binds) = expr.get("morphTargetBinds").and_then(|v| v.as_array()) {
                for bind in binds {
                    if let (Some(node), Some(index)) = (
                        bind.get("node")
                            .and_then(serde_json::Value::as_u64)
                            .map(|v| v as usize),
                        bind.get("index")
                            .and_then(serde_json::Value::as_u64)
                            .map(|v| v as usize),
                    ) {
                        let weight = bind
                            .get("weight")
                            .and_then(serde_json::Value::as_f64)
                            .unwrap_or(1.0) as f32;
                        def.morph_target_binds.push(MorphTargetBind {
                            node,
                            index,
                            weight,
                        });
                    }
                }
            }

            defs.push(def);
            parsed_count += 1;
        }
    }

    if parsed_count > 0 {
        tracing::info!("VRM {parsed_count} expression override definition(s) loaded");
    }

    defs
}

use crate::expression::{ExpressionLayer, MAX_MORPH_TARGETS_PER_PRIMITIVE};

impl ExpressionLayer {
    /// Apply per-expression override semantics to the current
    /// weight map, consuming the definitions parsed from the
    /// `VRMC_vrm.expressions` block.
    ///
    /// Call this every frame **after** writing per-frame weights
    /// (`set_expression` / `apply_weights`) and **before** the
    /// renderer reads the weight map.
    ///
    /// When `defs` is empty (no `VRMC_vrm.expressions` block) the
    /// call is a no-op.
    pub fn apply_overrides(&mut self, defs: &[ExpressionDefinition]) {
        apply_overrides(&mut self.weights, defs);

        self.morph_target_weights.clear();
        for def in defs {
            if let Some(&expr_weight) = self.weights.get(&def.name) {
                if expr_weight <= 0.0 {
                    continue;
                }
                let weight = if def.is_binary && expr_weight > 0.5 {
                    1.0
                } else {
                    expr_weight
                };
                for bind in &def.morph_target_binds {
                    if bind.index >= MAX_MORPH_TARGETS_PER_PRIMITIVE {
                        tracing::warn!(
                            "expression '{}' morph target bind index {} exceeds MAX_MORPH_TARGETS_PER_PRIMITIVE ({}); skipping",
                            def.name,
                            bind.index,
                            MAX_MORPH_TARGETS_PER_PRIMITIVE
                        );
                        continue;
                    }
                    let current = self
                        .morph_target_weights
                        .entry((bind.node, bind.index))
                        .or_insert(0.0);
                    *current = (*current + weight * bind.weight).clamp(0.0, 1.0);
                }
            }
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::default_trait_access,
    reason = "explicit Default for test fixture clarity"
)]
mod tests {
    use super::*;

    fn mk(name: &str) -> ExpressionName {
        ExpressionName::new(name)
    }

    fn mk_map(pairs: &[(&str, f32)]) -> BTreeMap<ExpressionName, f32> {
        pairs.iter().map(|(n, w)| (mk(n), *w)).collect()
    }

    fn def(
        name: &str,
        is_binary: bool,
        mouth: ExpressionOverrideType,
        blink: ExpressionOverrideType,
        look_at: ExpressionOverrideType,
    ) -> ExpressionDefinition {
        ExpressionDefinition {
            name: mk(name),
            is_binary,
            overrides: ExpressionOverrideSettings {
                mouth,
                blink,
                look_at,
            },
            morph_target_binds: Vec::new(),
        }
    }

    fn def_none(name: &str) -> ExpressionDefinition {
        ExpressionDefinition::new(name)
    }

    #[test]
    fn is_binary_clamps_above_half_to_one() {
        let mut w = mk_map(&[("happy", 0.51)]);
        let defs = vec![def(
            "happy",
            true,
            Default::default(),
            Default::default(),
            Default::default(),
        )];
        apply_overrides(&mut w, &defs);
        assert_eq!(w.get(&mk("happy")), Some(&1.0));
    }

    #[test]
    fn is_binary_clamps_below_half_to_zero() {
        let mut w = mk_map(&[("happy", 0.49)]);
        let defs = vec![def(
            "happy",
            true,
            Default::default(),
            Default::default(),
            Default::default(),
        )];
        apply_overrides(&mut w, &defs);
        assert_eq!(w.get(&mk("happy")), Some(&0.0));
    }

    #[test]
    fn is_binary_false_leaves_weight() {
        let mut w = mk_map(&[("happy", 0.75)]);
        let defs = vec![def(
            "happy",
            false,
            Default::default(),
            Default::default(),
            Default::default(),
        )];
        apply_overrides(&mut w, &defs);
        assert_eq!(w.get(&mk("happy")), Some(&0.75));
    }

    #[test]
    fn block_zeros_target_when_source_active() {
        let mut w = mk_map(&[("happy", 0.8), ("blink", 0.6)]);
        let defs = vec![
            def(
                "happy",
                false,
                Default::default(),
                ExpressionOverrideType::Block,
                Default::default(),
            ),
            def_none("blink"),
        ];
        apply_overrides(&mut w, &defs);
        assert_eq!(w.get(&mk("happy")), Some(&0.8));
        assert_eq!(w.get(&mk("blink")), Some(&0.0));
    }

    #[test]
    fn block_does_not_affect_target_when_source_inactive() {
        let mut w = mk_map(&[("happy", 0.0), ("blink", 0.6)]);
        let defs = vec![
            def(
                "happy",
                false,
                Default::default(),
                ExpressionOverrideType::Block,
                Default::default(),
            ),
            def_none("blink"),
        ];
        apply_overrides(&mut w, &defs);
        assert_eq!(w.get(&mk("blink")), Some(&0.6));
    }

    #[test]
    fn blend_attenuates_target_by_source_weight() {
        let mut w = mk_map(&[("happy", 0.5), ("blink", 0.8)]);
        let defs = vec![
            def(
                "happy",
                false,
                Default::default(),
                ExpressionOverrideType::Blend,
                Default::default(),
            ),
            def_none("blink"),
        ];
        apply_overrides(&mut w, &defs);
        // blink *= (1 − 0.5) = 0.8 * 0.5 = 0.4
        assert_eq!(w.get(&mk("blink")), Some(&0.4));
    }

    #[test]
    fn blend_sums_multiple_sources() {
        let mut w = mk_map(&[("happy", 0.3), ("angry", 0.4), ("blink", 0.8)]);
        let defs = vec![
            def(
                "happy",
                false,
                Default::default(),
                ExpressionOverrideType::Blend,
                Default::default(),
            ),
            def(
                "angry",
                false,
                Default::default(),
                ExpressionOverrideType::Blend,
                Default::default(),
            ),
            def_none("blink"),
        ];
        apply_overrides(&mut w, &defs);
        // blink *= (1 − saturate(0.3 + 0.4)) = (1 − 0.7) = 0.3
        // 0.8 * 0.3 = 0.24; f32 arithmetic has some rounding
        let val = w.get(&mk("blink")).copied().unwrap();
        assert!((val - 0.24).abs() < 1e-6, "expected 0.24, got {val}");
    }

    #[test]
    fn blend_clamped_to_zero_when_sum_ge_one() {
        let mut w = mk_map(&[("happy", 0.6), ("angry", 0.5), ("blink", 0.5)]);
        let defs = vec![
            def(
                "happy",
                false,
                Default::default(),
                ExpressionOverrideType::Blend,
                Default::default(),
            ),
            def(
                "angry",
                false,
                Default::default(),
                ExpressionOverrideType::Blend,
                Default::default(),
            ),
            def_none("blink"),
        ];
        apply_overrides(&mut w, &defs);
        // blend_sum = saturate(0.6 + 0.5) = 1.0 → blink *= 0
        assert_eq!(w.get(&mk("blink")), Some(&0.0));
    }

    #[test]
    fn block_wins_over_blend_when_both_affect_same_target() {
        let mut w = mk_map(&[("happy", 0.5), ("angry", 0.5), ("blink", 0.8)]);
        let defs = vec![
            def(
                "happy",
                false,
                Default::default(),
                ExpressionOverrideType::Blend,
                Default::default(),
            ),
            def(
                "angry",
                false,
                Default::default(),
                ExpressionOverrideType::Block,
                Default::default(),
            ),
            def_none("blink"),
        ];
        apply_overrides(&mut w, &defs);
        assert_eq!(w.get(&mk("blink")), Some(&0.0));
    }

    /// v1-aligned test: an emotion that sets `overrideBlink = block`
    /// must zero **every** blink-family expression — the synthetic
    /// `blink` and the per-side `blinkLeft` / `blinkRight`. The
    /// spec describes the blink family as a single overridable
    /// target group; missing a side blink would let a
    /// `happy`-suppressed character's left eye still blink.
    #[test]
    fn block_zeros_all_blink_family_members() {
        let mut w = mk_map(&[
            ("happy", 0.8),
            ("blink", 0.6),
            ("blinkLeft", 0.4),
            ("blinkRight", 0.4),
        ]);
        let defs = vec![
            def(
                "happy",
                false,
                Default::default(),
                ExpressionOverrideType::Block,
                Default::default(),
            ),
            def_none("blink"),
            def_none("blinkLeft"),
            def_none("blinkRight"),
        ];
        apply_overrides(&mut w, &defs);
        // happy is not a blink family member — stays.
        assert_eq!(w.get(&mk("happy")), Some(&0.8));
        assert_eq!(w.get(&mk("blink")), Some(&0.0));
        assert_eq!(w.get(&mk("blinkLeft")), Some(&0.0));
        assert_eq!(w.get(&mk("blinkRight")), Some(&0.0));
    }

    /// v1-aligned test: a `block` on the look-at family zeros
    /// `lookUp` / `lookDown` / `lookLeft` / `lookRight` together.
    /// The same-family block covers the whole `GAZE_TARGET_NAMES`
    /// set.
    #[test]
    fn block_zeros_all_gaze_family_members() {
        let mut w = mk_map(&[
            ("happy", 0.7),
            ("lookUp", 0.5),
            ("lookDown", 0.4),
            ("lookLeft", 0.3),
            ("lookRight", 0.2),
        ]);
        let defs = vec![
            def(
                "happy",
                false,
                Default::default(),
                Default::default(),
                ExpressionOverrideType::Block,
            ),
            def_none("lookUp"),
            def_none("lookDown"),
            def_none("lookLeft"),
            def_none("lookRight"),
        ];
        apply_overrides(&mut w, &defs);
        assert_eq!(w.get(&mk("happy")), Some(&0.7));
        assert_eq!(w.get(&mk("lookUp")), Some(&0.0));
        assert_eq!(w.get(&mk("lookDown")), Some(&0.0));
        assert_eq!(w.get(&mk("lookLeft")), Some(&0.0));
        assert_eq!(w.get(&mk("lookRight")), Some(&0.0));
    }

    #[test]
    fn same_kind_override_is_ignored() {
        let mut w = mk_map(&[("blink", 0.5)]);
        let defs = vec![def(
            "blink",
            false,
            Default::default(),
            ExpressionOverrideType::Block,
            Default::default(),
        )];
        apply_overrides(&mut w, &defs);
        assert_eq!(w.get(&mk("blink")), Some(&0.5));
    }

    #[test]
    fn is_binary_source_uses_binary_output_for_override() {
        let mut w = mk_map(&[("happy", 0.7), ("blink", 0.8)]);
        let defs = vec![
            def(
                "happy",
                true,
                Default::default(),
                ExpressionOverrideType::Blend,
                Default::default(),
            ),
            def_none("blink"),
        ];
        apply_overrides(&mut w, &defs);
        // happy.is_binary = true, weight 0.7 → binary = 1.0 (from isBinary clamp)
        // blink *= (1 − 1.0) = 0
        assert_eq!(w.get(&mk("blink")), Some(&0.0));
    }

    #[test]
    fn is_binary_source_below_half_gives_no_override() {
        let mut w = mk_map(&[("happy", 0.3), ("blink", 0.8)]);
        let defs = vec![
            def(
                "happy",
                true,
                Default::default(),
                ExpressionOverrideType::Block,
                Default::default(),
            ),
            def_none("blink"),
        ];
        apply_overrides(&mut w, &defs);
        // happy weight 0.3 → binary = 0.0 → no override
        assert_eq!(w.get(&mk("blink")), Some(&0.8));
    }

    #[test]
    fn is_binary_target_suppressed_when_overridden() {
        let mut w = mk_map(&[("happy", 0.6), ("blink", 0.5)]);
        let defs = vec![
            def(
                "happy",
                false,
                Default::default(),
                ExpressionOverrideType::Blend,
                Default::default(),
            ),
            def(
                "blink",
                true,
                Default::default(),
                Default::default(),
                Default::default(),
            ),
        ];
        apply_overrides(&mut w, &defs);
        // blink.is_binary = true + blend_sum > 0 → suppress to 0
        assert_eq!(w.get(&mk("blink")), Some(&0.0));
    }

    #[test]
    fn is_binary_target_suppressed_by_is_binary_source() {
        let mut w = mk_map(&[("happy", 0.7), ("blink", 0.6)]);
        let defs = vec![
            def(
                "happy",
                true,
                Default::default(),
                ExpressionOverrideType::Blend,
                Default::default(),
            ),
            def(
                "blink",
                true,
                Default::default(),
                Default::default(),
                Default::default(),
            ),
        ];
        apply_overrides(&mut w, &defs);
        // happy.is_binary weight 0.7 → binary 1.0 → blend_sum = 1.0
        // blink.is_binary = true + blend_sum > 0 → suppress
        assert_eq!(w.get(&mk("blink")), Some(&0.0));
    }

    #[test]
    fn override_mouth_affects_only_mouth_targets() {
        let mut w = mk_map(&[
            ("happy", 0.8),
            ("aa", 0.5),
            ("ih", 0.5),
            ("blink", 0.5),
            ("lookUp", 0.5),
        ]);
        let defs = vec![
            def(
                "happy",
                false,
                ExpressionOverrideType::Block,
                Default::default(),
                Default::default(),
            ),
            def_none("aa"),
            def_none("ih"),
            def_none("blink"),
            def_none("lookUp"),
        ];
        apply_overrides(&mut w, &defs);
        assert_eq!(w.get(&mk("aa")), Some(&0.0));
        assert_eq!(w.get(&mk("ih")), Some(&0.0));
        assert_eq!(w.get(&mk("blink")), Some(&0.5));
        assert_eq!(w.get(&mk("lookUp")), Some(&0.5));
    }

    #[test]
    fn override_look_at_affects_only_gaze_targets() {
        let mut w = mk_map(&[
            ("happy", 0.8),
            ("lookUp", 0.5),
            ("lookDown", 0.5),
            ("blink", 0.5),
            ("aa", 0.5),
        ]);
        let defs = vec![
            def(
                "happy",
                false,
                Default::default(),
                Default::default(),
                ExpressionOverrideType::Block,
            ),
            def_none("lookUp"),
            def_none("lookDown"),
            def_none("blink"),
            def_none("aa"),
        ];
        apply_overrides(&mut w, &defs);
        assert_eq!(w.get(&mk("lookUp")), Some(&0.0));
        assert_eq!(w.get(&mk("lookDown")), Some(&0.0));
        assert_eq!(w.get(&mk("blink")), Some(&0.5));
        assert_eq!(w.get(&mk("aa")), Some(&0.5));
    }

    #[test]
    fn blink_override_affects_blink_left_and_right() {
        let mut w = mk_map(&[
            ("happy", 0.8),
            ("blink", 0.5),
            ("blinkLeft", 0.5),
            ("blinkRight", 0.5),
        ]);
        let defs = vec![
            def(
                "happy",
                false,
                Default::default(),
                ExpressionOverrideType::Block,
                Default::default(),
            ),
            def_none("blink"),
            def_none("blinkLeft"),
            def_none("blinkRight"),
        ];
        apply_overrides(&mut w, &defs);
        assert_eq!(w.get(&mk("blink")), Some(&0.0));
        assert_eq!(w.get(&mk("blinkLeft")), Some(&0.0));
        assert_eq!(w.get(&mk("blinkRight")), Some(&0.0));
    }

    #[test]
    fn empty_defs_is_no_op() {
        let mut w = mk_map(&[("happy", 0.5), ("blink", 0.5)]);
        let defs: Vec<ExpressionDefinition> = vec![];
        apply_overrides(&mut w, &defs);
        assert_eq!(w.get(&mk("happy")), Some(&0.5));
        assert_eq!(w.get(&mk("blink")), Some(&0.5));
    }

    #[test]
    fn empty_weights_is_no_op() {
        let mut w: BTreeMap<ExpressionName, f32> = BTreeMap::new();
        let defs = vec![def(
            "happy",
            false,
            Default::default(),
            ExpressionOverrideType::Block,
            Default::default(),
        )];
        apply_overrides(&mut w, &defs);
        assert!(w.is_empty());
    }

    #[test]
    fn custom_expression_can_override_procedural() {
        let mut w = mk_map(&[("custom_smile", 0.8), ("blink", 0.5)]);
        let defs = vec![
            def(
                "custom_smile",
                false,
                Default::default(),
                ExpressionOverrideType::Block,
                Default::default(),
            ),
            def_none("blink"),
        ];
        apply_overrides(&mut w, &defs);
        assert_eq!(w.get(&mk("blink")), Some(&0.0));
    }

    #[test]
    fn override_type_from_json_str_block() {
        assert_eq!(
            ExpressionOverrideType::from_json_str("block"),
            ExpressionOverrideType::Block
        );
    }

    #[test]
    fn override_type_from_json_str_blend() {
        assert_eq!(
            ExpressionOverrideType::from_json_str("blend"),
            ExpressionOverrideType::Blend
        );
    }

    #[test]
    fn override_type_from_json_str_none() {
        assert_eq!(
            ExpressionOverrideType::from_json_str("none"),
            ExpressionOverrideType::None
        );
    }

    #[test]
    fn override_type_from_json_str_unknown_falls_back_to_none() {
        assert_eq!(
            ExpressionOverrideType::from_json_str("garbage"),
            ExpressionOverrideType::None
        );
    }

    #[test]
    fn is_procedural_recognises_all_targets() {
        for &name in MOUTH_TARGET_NAMES
            .iter()
            .chain(BLINK_TARGET_NAMES.iter())
            .chain(GAZE_TARGET_NAMES.iter())
        {
            assert!(is_procedural(name), "{name} should be procedural");
        }
        assert!(!is_procedural("happy"));
        assert!(!is_procedural("sad"));
    }

    /// Wrap a `VRMC_vrm` extension block in a minimal glTF 2.0
    /// document and parse it. Mirrors the pattern in the
    /// `look_at.rs` tests.
    fn gltf_from_vrmc(vrmc: &serde_json::Value) -> gltf::Gltf {
        let json = serde_json::json!({
            "asset": { "version": "2.0" },
            "extensionsUsed": ["VRMC_vrm"],
            "extensions": vrmc.clone(),
        });
        let bytes = serde_json::to_vec(&json).unwrap();
        gltf::Gltf::from_slice(&bytes).expect("gltf parse")
    }

    #[test]
    fn load_expression_overrides_full_block() {
        use serde_json::json;
        let vrmc = json!({
            "VRMC_vrm": {
                "expressions": {
                    "preset": {
                        "happy": {
                            "isBinary": false,
                            "overrideMouth": "block",
                            "overrideBlink": "blend",
                            "overrideLookAt": "none",
                            "morphTargetBinds": []
                        },
                        "blink": {
                            "isBinary": true,
                            "overrideMouth": "none",
                            "overrideBlink": "none",
                            "overrideLookAt": "none",
                            "morphTargetBinds": []
                        }
                    },
                    "custom": {
                        "custom_wink": {
                            "isBinary": false,
                            "overrideMouth": "none",
                            "overrideBlink": "block",
                            "overrideLookAt": "none",
                            "morphTargetBinds": []
                        }
                    }
                }
            }
        });
        let gltf = gltf_from_vrmc(&vrmc);
        let defs = load_expression_overrides(&gltf);
        assert_eq!(defs.len(), 3);

        let happy = defs.iter().find(|d| d.name.as_str() == "happy").unwrap();
        assert!(!happy.is_binary);
        assert_eq!(happy.overrides.mouth, ExpressionOverrideType::Block);
        assert_eq!(happy.overrides.blink, ExpressionOverrideType::Blend);
        assert_eq!(happy.overrides.look_at, ExpressionOverrideType::None);

        let blink = defs.iter().find(|d| d.name.as_str() == "blink").unwrap();
        assert!(blink.is_binary);
        assert_eq!(blink.overrides.blink, ExpressionOverrideType::None);

        let custom = defs
            .iter()
            .find(|d| d.name.as_str() == "custom_wink")
            .unwrap();
        assert!(!custom.is_binary);
        assert_eq!(custom.overrides.blink, ExpressionOverrideType::Block);
    }

    #[test]
    fn load_expression_overrides_missing_fields_default() {
        use serde_json::json;
        let vrmc = json!({
            "VRMC_vrm": {
                "expressions": {
                    "preset": {
                        "minimal": {
                            "morphTargetBinds": []
                        }
                    }
                }
            }
        });
        let gltf = gltf_from_vrmc(&vrmc);
        let defs = load_expression_overrides(&gltf);
        assert_eq!(defs.len(), 1);
        let d = &defs[0];
        assert_eq!(d.name.as_str(), "minimal");
        assert!(!d.is_binary);
        assert_eq!(d.overrides, ExpressionOverrideSettings::default());
    }

    #[test]
    fn load_expression_overrides_missing_block_returns_empty() {
        use serde_json::json;
        let vrmc = json!({ "VRMC_vrm": {} });
        let gltf = gltf_from_vrmc(&vrmc);
        let defs = load_expression_overrides(&gltf);
        assert!(defs.is_empty());
    }

    #[test]
    fn load_expression_overrides_unknown_override_string_falls_back() {
        use serde_json::json;
        let vrmc = json!({
            "VRMC_vrm": {
                "expressions": {
                    "preset": {
                        "bad": {
                            "isBinary": false,
                            "overrideBlink": "garbage",
                            "morphTargetBinds": []
                        }
                    }
                }
            }
        });
        let gltf = gltf_from_vrmc(&vrmc);
        let defs = load_expression_overrides(&gltf);
        assert_eq!(defs[0].overrides.blink, ExpressionOverrideType::None);
    }

    #[test]
    fn load_expression_overrides_is_binary_true_parsed() {
        use serde_json::json;
        let vrmc = json!({
            "VRMC_vrm": {
                "expressions": {
                    "preset": {
                        "blink": {
                            "isBinary": true,
                            "morphTargetBinds": []
                        }
                    }
                }
            }
        });
        let gltf = gltf_from_vrmc(&vrmc);
        let defs = load_expression_overrides(&gltf);
        assert!(defs[0].is_binary);
    }

    #[test]
    fn apply_overrides_on_direct_weight_map() {
        let mut w = mk_map(&[("happy", 0.8), ("blink", 0.5)]);
        let defs = vec![
            def(
                "happy",
                false,
                Default::default(),
                ExpressionOverrideType::Block,
                Default::default(),
            ),
            def_none("blink"),
        ];
        apply_overrides(&mut w, &defs);
        assert_eq!(w.get(&mk("blink")), Some(&0.0));
    }
}
