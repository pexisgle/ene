//! Identity Kernel types compiled from `CCv3` character cards (#82).

/// Compiled immutable identity block for prompt injection.
#[derive(Debug, Clone)]
pub struct IdentityKernel {
    /// Character display name.
    pub name: String,
    /// Rendered identity kernel text (always injected first; never truncated).
    pub text: String,
    /// Post-history instructions kept out of the kernel body for PHI / output sections.
    pub post_history_instructions: Option<String>,
}

impl IdentityKernel {
    /// Whether post-history instructions are available for downstream prompt sections.
    #[must_use]
    pub fn has_post_history_instructions(&self) -> bool {
        self.post_history_instructions
            .as_ref()
            .is_some_and(|s| !s.trim().is_empty())
    }
}
