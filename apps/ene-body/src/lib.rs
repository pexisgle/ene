pub mod error;
pub mod ipc;
pub mod motion;
pub mod render;
pub mod run;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod vrm;
pub mod window;

pub use error::BodyError;
pub use run::{RunOptions, parse_endpoint, run, run_with_io};

#[cfg(test)]
mod contract_tests {
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
