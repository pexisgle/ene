//! Expression Compositor: merges character-card expression weights
//! with runtime expression overrides (#132).
//!
//! ## Data flow
//!
//! 1. **Base layer**: card-defined expression blendshape maps
//!    (e.g. `"happy" → { "happy": 0.8, "mouthSmile": 0.5, "browRaise": 0.3 }`).
//!    Multiple named expressions can be loaded; one is selected as
//!    the active base.
//!
//! 2. **Override layer**: runtime cues from the performance pipeline
//!    that override specific blendshape weights.
//!
//! 3. **Compose**: overrides are merged on top of the selected
//!    base, with overrides taking precedence for matching keys.
//!
//! This module lives in `ene-vrm` and is independent of `ene-mind`
//! and `ene-config`. The desktop app bridges `ResolvedExpression`
//! and `PerformanceCue` into the simple maps expected here.

use std::collections::HashMap;

/// A named expression weight-map from the character card.
#[derive(Debug, Clone)]
pub struct CardExpression {
    /// Expression name (e.g. `"happy"`, `"neutral"`).
    pub name: String,
    /// Blend-shape → weight map (e.g. `"happy" → 0.8`).
    pub weights: HashMap<String, f32>,
}

/// Merges card-defined expression weights with runtime overrides.
#[derive(Debug, Default)]
pub struct ExpressionCompositor {
    /// Loaded card expressions, keyed by name.
    card_expressions: HashMap<String, CardExpression>,
    /// Currently selected active expression name.
    active_base: Option<String>,
    /// Runtime overrides (blend-shape name → weight).
    overrides: HashMap<String, f32>,
}

impl ExpressionCompositor {
    /// Load a card-defined expression into the compositor.
    pub fn load_card_expression(&mut self, name: String, weights: HashMap<String, f32>) {
        self.card_expressions
            .insert(name.clone(), CardExpression { name, weights });
    }

    /// Select which card expression to use as the base layer.
    /// Pass `None` to clear the selection.
    pub fn set_active_expression<S: AsRef<str>>(&mut self, name: Option<S>) {
        self.active_base = name.map(|n| n.as_ref().to_string());
    }

    /// Set a runtime override for a specific blend-shape.
    ///
    /// Overrides take precedence over the active card expression
    /// for the same blend-shape key.
    pub fn set_override(&mut self, blend_shape: String, weight: f32) {
        self.overrides.insert(blend_shape, weight.clamp(0.0, 1.0));
    }

    /// Remove a single runtime override.
    pub fn remove_override(&mut self, blend_shape: &str) {
        self.overrides.remove(blend_shape);
    }

    /// Clear all runtime overrides.
    pub fn clear_overrides(&mut self) {
        self.overrides.clear();
    }

    /// Clear all loaded card expressions.
    pub fn clear_card_expressions(&mut self) {
        self.card_expressions.clear();
        self.active_base = None;
    }

    /// Compose the final weight map by merging the active card
    /// expression's weights with runtime overrides.
    ///
    /// Override keys win over card keys. All weights are clamped
    /// to `[0, 1]`.
    #[must_use]
    pub fn compose(&self) -> HashMap<String, f32> {
        let mut result = HashMap::new();

        if let Some(ref base_name) = self.active_base
            && let Some(card_expr) = self.card_expressions.get(base_name)
        {
            for (key, &weight) in &card_expr.weights {
                result.insert(key.clone(), weight.clamp(0.0, 1.0));
            }
        }

        for (key, &weight) in &self.overrides {
            result.insert(key.clone(), weight.clamp(0.0, 1.0));
        }

        result
    }

    /// Returns the name of the currently active card expression.
    #[must_use]
    pub fn active_expression_name(&self) -> Option<&str> {
        self.active_base.as_deref()
    }

    /// Returns the names of all loaded card expressions.
    #[must_use]
    pub fn loaded_expression_names(&self) -> Vec<&str> {
        self.card_expressions
            .keys()
            .map(std::string::String::as_str)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn happy_card() -> CardExpression {
        CardExpression {
            name: "happy".into(),
            weights: [("happy".into(), 0.8), ("mouthSmile".into(), 0.5)]
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn base_only_composes_card_weights() {
        let mut c = ExpressionCompositor::default();
        c.load_card_expression("happy".into(), happy_card().weights);
        c.set_active_expression(Some("happy"));
        let result = c.compose();
        assert_eq!(result.get("happy"), Some(&0.8));
        assert_eq!(result.get("mouthSmile"), Some(&0.5));
    }

    #[test]
    fn override_wins_over_base() {
        let mut c = ExpressionCompositor::default();
        c.load_card_expression("happy".into(), happy_card().weights);
        c.set_active_expression(Some("happy"));
        c.set_override("happy".into(), 1.0);
        let result = c.compose();
        assert_eq!(result.get("happy"), Some(&1.0));
        assert_eq!(result.get("mouthSmile"), Some(&0.5));
    }

    #[test]
    fn override_without_base() {
        let mut c = ExpressionCompositor::default();
        c.set_override("custom".into(), 0.7);
        let result = c.compose();
        assert_eq!(result.get("custom"), Some(&0.7));
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn clear_overrides_restores_base() {
        let mut c = ExpressionCompositor::default();
        c.load_card_expression("happy".into(), happy_card().weights);
        c.set_active_expression(Some("happy"));
        c.set_override("happy".into(), 0.3);
        c.clear_overrides();
        let result = c.compose();
        assert_eq!(result.get("happy"), Some(&0.8));
    }

    #[test]
    fn switching_base_changes_output() {
        let mut c = ExpressionCompositor::default();
        c.load_card_expression("happy".into(), happy_card().weights);
        c.load_card_expression(
            "neutral".into(),
            [("neutral".into(), 0.0)].into_iter().collect(),
        );
        c.set_active_expression(Some("happy"));
        assert_eq!(c.compose().get("happy"), Some(&0.8));
        c.set_active_expression(Some("neutral"));
        assert_eq!(c.compose().get("neutral"), Some(&0.0));
        assert_eq!(c.compose().get("happy"), None);
    }

    #[test]
    fn weights_clamped_to_range() {
        let mut c = ExpressionCompositor::default();
        c.set_override("too_high".into(), 1.5);
        c.set_override("too_low".into(), -0.5);
        let result = c.compose();
        assert_eq!(result.get("too_high"), Some(&1.0));
        assert_eq!(result.get("too_low"), Some(&0.0));
    }

    #[test]
    fn remove_single_override() {
        let mut c = ExpressionCompositor::default();
        c.set_override("a".into(), 0.5);
        c.set_override("b".into(), 0.6);
        c.remove_override("a");
        let result = c.compose();
        assert_eq!(result.get("a"), None);
        assert_eq!(result.get("b"), Some(&0.6));
    }

    #[test]
    fn no_active_base_returns_only_overrides() {
        let mut c = ExpressionCompositor::default();
        c.load_card_expression("happy".into(), happy_card().weights);
        c.set_active_expression(None::<&str>);
        c.set_override("custom".into(), 0.5);
        let result = c.compose();
        assert_eq!(result.len(), 1);
        assert_eq!(result.get("custom"), Some(&0.5));
    }

    #[test]
    fn loaded_expression_names_returns_all() {
        let mut c = ExpressionCompositor::default();
        c.load_card_expression("happy".into(), HashMap::new());
        c.load_card_expression("neutral".into(), HashMap::new());
        let mut names = c.loaded_expression_names();
        names.sort();
        assert_eq!(names, vec!["happy", "neutral"]);
    }
}
