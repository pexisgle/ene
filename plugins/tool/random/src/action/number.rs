use crate::error::RandomError;
use ene_plugin::prelude::*;

/// Generates a random number.
///
/// Float mode samples uniformly from the half-open interval
/// `[min, max)`; integer mode samples uniformly from the closed
/// interval `[ceil(min), floor(max)]`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "random",
    name = "number",
    summary = "Generate a random number within a range.",
    description = "Generates a random number between min and max. With integer=false (default) the result is a float in [min, max) (min included, max excluded). With integer=true the result is a whole number in [min, max] (both ends included, bounds are rounded inward with ceil/floor). Both bounds must be finite and min must be less than max in float mode; an integer range with no whole numbers is an error.",
    category = "Utility",
    keywords_primary = "random, number, integer, float, dice, roll, lottery",
    side_effects = "ReadOnly"
)]
pub struct NumberAction {
    /// Lower bound of the range. Defaults to 0.
    #[serde(default)]
    min: f64,
    /// Upper bound of the range. Defaults to 1.
    #[serde(default = "default_max")]
    max: f64,
    /// When true, return a whole number instead of a float.
    #[serde(default)]
    integer: bool,
}

const fn default_max() -> f64 {
    1.0
}

impl NumberAction {
    async fn run(&self) -> Result<String, ToolError> {
        let value = if self.integer {
            random_integer(self.min, self.max)?.to_string()
        } else {
            random_float(self.min, self.max)?.to_string()
        };
        Ok(value)
    }
}

/// Samples a float uniformly from `[min, max)`.
fn random_float(min: f64, max: f64) -> Result<f64, RandomError> {
    if !min.is_finite() || !max.is_finite() {
        return Err(RandomError::NonFiniteBound { min, max });
    }
    if min >= max {
        return Err(RandomError::EmptyFloatRange { min, max });
    }
    Ok(rand::random_range(min..max))
}

/// Samples an integer uniformly from `[ceil(min), floor(max)]`.
fn random_integer(min: f64, max: f64) -> Result<i64, RandomError> {
    if !min.is_finite() || !max.is_finite() {
        return Err(RandomError::NonFiniteBound { min, max });
    }
    let lo = min.ceil() as i64;
    let hi = max.floor() as i64;
    if lo > hi {
        return Err(RandomError::EmptyIntRange { lo, hi });
    }
    Ok(rand::random_range(lo..=hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_stays_within_half_open_range() {
        for _ in 0..200 {
            let value = random_float(-5.0, 3.0).unwrap();
            assert!((-5.0..3.0).contains(&value), "{value} outside [-5, 3)");
        }
    }

    #[test]
    fn float_defaults_to_unit_range() {
        let action: NumberAction = serde_json::from_str("{}").unwrap();
        let value = random_float(action.min, action.max).unwrap();
        assert!((0.0..1.0).contains(&value), "{value} outside [0, 1)");
    }

    #[test]
    fn float_rejects_empty_and_inverted_ranges() {
        assert!(matches!(
            random_float(1.0, 1.0),
            Err(RandomError::EmptyFloatRange { .. })
        ));
        assert!(matches!(
            random_float(2.0, 1.0),
            Err(RandomError::EmptyFloatRange { .. })
        ));
    }

    #[test]
    fn float_rejects_non_finite_bounds() {
        assert!(matches!(
            random_float(f64::NAN, 1.0),
            Err(RandomError::NonFiniteBound { .. })
        ));
        assert!(matches!(
            random_float(0.0, f64::INFINITY),
            Err(RandomError::NonFiniteBound { .. })
        ));
    }

    #[test]
    fn integer_stays_within_closed_range() {
        for _ in 0..200 {
            let value = random_integer(1.0, 6.0).unwrap();
            assert!((1..=6).contains(&value), "{value} outside [1, 6]");
        }
    }

    #[test]
    fn integer_rounds_fractional_bounds_inward() {
        for _ in 0..200 {
            let value = random_integer(1.2, 5.8).unwrap();
            assert!((2..=5).contains(&value), "{value} outside [2, 5]");
        }
    }

    #[test]
    fn integer_single_value_range_returns_that_value() {
        for _ in 0..20 {
            assert_eq!(random_integer(3.0, 3.0).unwrap(), 3);
        }
    }

    #[test]
    fn integer_rejects_range_without_whole_numbers() {
        assert!(matches!(
            random_integer(2.5, 2.5),
            Err(RandomError::EmptyIntRange { .. })
        ));
        assert!(matches!(
            random_integer(4.0, 3.0),
            Err(RandomError::EmptyIntRange { .. })
        ));
    }

    #[test]
    fn integer_rejects_non_finite_bounds() {
        assert!(matches!(
            random_integer(f64::NEG_INFINITY, 1.0),
            Err(RandomError::NonFiniteBound { .. })
        ));
    }

    #[test]
    fn spec_name_and_parameters() {
        let action = NumberAction::default();
        let spec = NumberAction::spec();
        assert_eq!(action.name(), "random.number");
        assert_eq!(spec.name.as_str(), "random.number");
        let props = spec.parameters.get("properties").unwrap();
        assert!(props.get("min").is_some());
        assert!(props.get("max").is_some());
        assert!(props.get("integer").is_some());
    }
}
