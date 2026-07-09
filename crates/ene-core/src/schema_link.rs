//! Schema-registration link for `ene-cognition` config types (issue #95).
//!
//! `ene-config`'s `define_config!` macro registers each config section in
//! a process-global schema registry via a `#[ctor::ctor(unsafe)]` block
//! that runs at startup — but only if the crate defining that section is
//! actually linked into the final binary. `ene-core` is the one dependency
//! shared by every consumer (`ene-cli`, `ene-desktop`), so re-exporting
//! `ene_cognition::CognitionConfig` (and its sub-types) here forces
//! `ene-cognition` to be linked into every `ene-core` consumer, which in
//! turn fires its `ctor` block and registers the `cognition` section.
//!
//! **This module is not general application API.** Its sole purpose is
//! to keep the linker from dropping `ene-cognition`'s schema-registration
//! side effect. Application code that wants these types for their own
//! sake (e.g. reading `cognition.context.max_prompt_tokens`) should import
//! them directly from `ene-cognition`; the re-exports at the `ene-core`
//! crate root exist only so that pre-existing `ene_core::CognitionConfig`
//! -style imports keep compiling.
//!
//! Without this module (or an equivalent hard dependency), the schema
//! generator would never see the `cognition` section and the JSON schema
//! shipped to users would silently be missing the entire cognitive-runtime
//! configuration block.

/// Character-memory compilation settings (re-exported from `ene-cognition`).
#[doc(no_inline)]
pub use ene_cognition::CharacterMemoryConfig;
/// Cognitive runtime configuration (re-exported from `ene-cognition`).
///
/// This is the top-level `cognition` section in `settings.json`. Reading
/// it via [`ene_config::EneConfig::get_section`] returns the macro-defined
/// defaults when the key is absent.
#[doc(no_inline)]
pub use ene_cognition::CognitionConfig;
/// Memory extraction / retention sub-config (re-exported from `ene-cognition`).
#[doc(no_inline)]
pub use ene_cognition::CognitionMemoryConfig;
/// Context budget sub-config (re-exported from `ene-cognition`).
#[doc(no_inline)]
pub use ene_cognition::ContextConfig;
/// Emotion engine sub-config (re-exported from `ene-cognition`).
#[doc(no_inline)]
pub use ene_cognition::EmotionConfig;
