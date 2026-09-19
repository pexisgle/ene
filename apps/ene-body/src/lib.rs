//! VRM overlay child (`apps/ene-body`).
//!
//! Modules match first-party-desktop §6: [`window`], [`vrm`], [`render`],
//! [`ipc`]. This crate does not depend on Host, `ene-api`, `ene-client`,
//! credentials, or Task commands.

pub mod error;
pub mod ipc;
pub mod render;
pub mod run;
pub mod vrm;
pub mod window;

pub use error::BodyError;
pub use ipc::{BodyToParent, ParentToBody};
pub use run::{IpcEndpoint, RunOptions, parse_endpoint, run, run_with_io};

#[cfg(test)]
mod contract_tests {
    #[test]
    fn package_name_is_ene_body_not_ene_vrm() {
        let manifest = include_str!("../Cargo.toml");
        assert!(
            manifest.contains("name = \"ene-body\""),
            "crate name must be ene-body"
        );
        assert!(
            !manifest.contains("ene-vrm"),
            "crate name ene-vrm is banned"
        );
    }

    #[test]
    fn manifest_does_not_depend_on_host_or_client_protocol() {
        let manifest = include_str!("../Cargo.toml");
        for banned in [
            "ene-api",
            "ene-client",
            "ene-core",
            "ene-local-control",
            "ene-credential",
            "ene-plugin-ipc",
        ] {
            assert!(
                !manifest.contains(banned),
                "ene-body must not depend on {banned}"
            );
        }
    }
}
