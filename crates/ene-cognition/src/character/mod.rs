//! Character processing: CCv3 compilation and Identity Kernel (#82 seed).

mod kernel;

pub use kernel::{IdentityKernel, compile_identity_kernel};

/// Character processing: CCv3 compilation, Identity Kernel, lorebook indexing.
pub struct CharacterProcessor;

impl CharacterProcessor {
    /// Compile the identity kernel for a character card.
    #[must_use]
    pub fn compile_kernel(card: &ene_config::CharacterCardV3, user_name: &str) -> IdentityKernel {
        compile_identity_kernel(card, user_name)
    }
}
