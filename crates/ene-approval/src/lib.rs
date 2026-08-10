//! # ene-approval
//!
//! Two-layer plugin permission model.
//!
//! Layer 1 is the **manifest layer**: a signed plugin manifest declares the
//! *maximum* set of capabilities a plugin may request (logical FS slots,
//! fixed origins, `dynamic_web`, artifact requirements, host services, side
//! effects). The manifest layer is enforced by the host before any request
//! reaches an [`ApprovalCategory`] — a capability that is not declared can
//! never be approved.
//!
//! Layer 2 is the **approval layer** implemented here: host-side policies
//! decide whether a declared capability request is automatically allowed,
//! automatically denied, or needs interactive confirmation.
//!
//! Resolution order (see [`ApprovalResolver::resolve`]):
//!
//! 1. Mandatory security constraints (signature, hash, size, SSRF block) —
//!    enforced outside this crate; a violation never reaches the resolver.
//! 2. Per-plugin override (wins over the global policy).
//! 3. Global policy.
//! 4. `Ask` (interactive confirmation; headless consumers fail safe to deny).
//!
//! Everything the resolver decides — including automatic allow/deny — is
//! recorded by [`AuditLog`] so an operator can reconstruct which rule
//! applied to which request. Secrets, request bodies, and user file contents
//! are never recorded.

#![warn(missing_docs)]
#![cfg_attr(
    test,
    expect(
        clippy::expect_used,
        reason = "unit tests use expect for concise assertions"
    )
)]

/// Append-only audit log for every resolved request.
pub mod audit;
/// The operation categories the approval system can gate.
pub mod category;
/// The manifest layer: what a plugin may request.
pub mod manifest;
/// Configured and resolved approval modes.
pub mod mode;
/// Global/per-plugin policies and the resolution order.
pub mod policy;
/// The request object exchanged with confirmation UI.
pub mod request;

pub use audit::{AuditLog, AuditLogEntry};
pub use category::{ALL_CATEGORIES, ApprovalCategory, HIGH_RISK_CATEGORIES};
pub use manifest::{
    FsSlotDecl, ManifestPermission, ManifestSideEffects, OriginDecl, PluginManifest,
    ResourceLimits, SignedManifest, canonical_manifest_bytes, manifest_sha256,
};
pub use mode::{ApprovalMode, ResolvedMode};
pub use policy::{
    ApprovalPolicy, ApprovalResolver, PluginApprovalPolicy, Resolution, ResolutionReason,
};
pub use request::ApprovalRequest;
