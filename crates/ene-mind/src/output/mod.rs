//! Output Arbiter: expression validation, hysteresis, output mapping (#89).

mod arbiter;
pub mod performance;
pub mod performance_arbiter;
pub mod types;

pub use arbiter::{affect_to_expression, normalize_expression};
pub use performance::{CueSource, MotionLayer, PerfKind, PerformanceCue};
#[doc(no_inline)]
pub use performance_arbiter::{PerformanceArbiter, cue_source_priority};
pub use types::{ExpressionDecision, ExpressionInput, ExpressionSource};

use crate::config::EmotionConfig;

use self::arbiter::resolve_expression;

/// Output Arbiter: expression validation, hysteresis, output mapping.
pub struct OutputArbiter;

impl OutputArbiter {
    /// Resolve the final character expression for a completed assistant turn.
    pub fn resolve(
        &self,
        config: &EmotionConfig,
        input: &ExpressionInput<'_>,
    ) -> ExpressionDecision {
        let decision = resolve_expression(config, input);
        tracing::debug!(
            component = "OutputArbiter",
            expression = %decision.expression,
            source = decision.source.as_str(),
            reason = %decision.reason,
            "Expression resolved"
        );
        decision
    }
}
