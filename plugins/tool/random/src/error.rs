use ene_plugin_proto::ToolError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RandomError {
    #[error("range bounds must be finite numbers, got min={min}, max={max}")]
    NonFiniteBound { min: f64, max: f64 },
    #[error("min must be less than max for float ranges, got min={min}, max={max}")]
    EmptyFloatRange { min: f64, max: f64 },
    #[error("integer range is empty: ceil(min)={lo} > floor(max)={hi}")]
    EmptyIntRange { lo: f64, hi: f64 },
    #[error("integer range exceeds i64 bounds: ceil(min)={lo}, floor(max)={hi}")]
    IntRangeOutOfBounds { lo: f64, hi: f64 },
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
