//! Curated "Supported API" entry points for `ene-vrm` consumers (primarily `ene-desktop`).
//!
//! `glob`-import this module (`use ene_vrm::prelude::*;`) to pull in the small, stable set of
//! types and functions a typical host application needs: load a model, render it, animate it,
//! drive look-at, and read/write expression weights.
//!
//! ```
//! use ene_vrm::prelude::*;
//! ```
//!
//! Everything else in the crate (spring bones, MToon material internals, node constraints,
//! debug rendering, raw glTF sub-parsers, retargeting helpers, …) remains `pub` for advanced
//! hosts that need it, but is intentionally left out of this module — see
//! `docs/reference/api/ene-vrm.md` ("Supported API" vs "Internal").

#[doc(no_inline)]
pub use crate::animation::{VrmaAsset, VrmaClip, VrmaFrame, VrmaPlayer, evaluate_clip, load_vrma};
#[doc(no_inline)]
pub use crate::error::{VrmError, VrmResult};
#[doc(no_inline)]
pub use crate::expression::{ExpressionLayer, ExpressionName};
#[doc(no_inline)]
pub use crate::expression_compositor::{CardExpression, ExpressionCompositor};
#[doc(no_inline)]
pub use crate::layer_composer::ComposedFrame;
#[doc(no_inline)]
pub use crate::loader::load_vrm;
#[doc(no_inline)]
pub use crate::look_at::{LookAtBoneOutput, LookAtEvaluator, LookAtOutput, LookAtProperties};
#[doc(no_inline)]
pub use crate::model::VrmModel;
#[doc(no_inline)]
pub use crate::renderer::VrmRenderer;
pub use ene_config::MotionLayer;
