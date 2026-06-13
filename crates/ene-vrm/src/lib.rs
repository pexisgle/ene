//! `ene-vrm` — VRM 1.0 character renderer for the ene desktop app.
//!
//! This crate is the platform-agnostic replacement for `bevy_vrm1`. It is built
//! directly on top of `wgpu` 27 and the `gltf` 1.4 crate. The full design is
//! documented in `docs/architecture/wgpu-migration.md`.
//!
//! PR1 only ships a stub. Subsequent PRs will land:
//! - PR3: glTF/VRM loading, MToon WGSL pipeline, skinning.
//! - PR4: expressions, look-at, body tracking.
//! - PR5+: VRMA, spring-bone.

#![warn(missing_docs)]

/// Returns the crate version. Useful for diagnostics and the `about` panel.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty() {
        assert!(!version().is_empty());
    }
}
