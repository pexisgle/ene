use ene_plugin_proto::ToolError;
use thiserror::Error;

/// Argument validation failure for a `random.*` action.
#[derive(Debug, Error)]
pub enum RandomError {
    /// A range bound was NaN or infinite.
    #[error("range bounds must be finite numbers, got min={min}, max={max}")]
    NonFiniteBound {
        /// Lower bound as passed by the caller.
        min: f64,
        /// Upper bound as passed by the caller.
        max: f64,
    },
    /// The float range has no interior points.
    #[error("min must be less than max for float ranges, got min={min}, max={max}")]
    EmptyFloatRange {
        /// Lower bound as passed by the caller.
        min: f64,
        /// Upper bound as passed by the caller.
        max: f64,
    },
    /// The integer range contains no whole numbers.
    #[error("integer range is empty: ceil(min)={lo} > floor(max)={hi}")]
    EmptyIntRange {
        /// Smallest whole number in the range.
        lo: f64,
        /// Largest whole number in the range.
        hi: f64,
    },
    /// The integer range lies outside the `i64` representable range.
    #[error("integer range exceeds i64 bounds: ceil(min)={lo}, floor(max)={hi}")]
    IntRangeOutOfBounds {
        /// Smallest whole number in the range.
        lo: f64,
        /// Largest whole number in the range.
        hi: f64,
    },
    /// `random.pick` was given no options.
    #[error("options must not be empty")]
    EmptyOptions,
}

impl From<RandomError> for ToolError {
    fn from(e: RandomError) -> Self {
        ToolError::InvalidArguments {
            message: e.to_string(),
        }
    }
}
