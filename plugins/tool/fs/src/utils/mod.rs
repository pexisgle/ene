pub mod broker;
pub mod permission;
pub mod sandbox;

use std::sync::{Arc, RwLock};

/// Shared reference to an optional [`Sandbox`](sandbox::Sandbox).
///
/// Used by every action module to hold a lazily-resolved sandbox
/// instance that is populated by the provider via hooks.
pub type SandboxRef = Arc<RwLock<Option<Arc<sandbox::Sandbox>>>>;

/// Creates a default (empty) [`SandboxRef`].
pub fn default_sandbox() -> SandboxRef {
    Arc::new(RwLock::new(None))
}

/// Resolves the sandbox from a [`SandboxRef`], falling back to a
/// default [`Sandbox`](sandbox::Sandbox) if the ref is `None`.
pub fn resolve_sandbox(sandbox_ref: &SandboxRef) -> Arc<sandbox::Sandbox> {
    let guard = sandbox_ref
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard
        .clone()
        .unwrap_or_else(|| Arc::new(sandbox::Sandbox::new(sandbox::SandboxConfig::default())))
}
