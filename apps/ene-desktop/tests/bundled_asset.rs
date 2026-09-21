//! Bundled VRM 1.0 sample asset guard: `assets/seed-san.vrm` must stay an
//! unmodified Seed-san sample with the features Body needs.

#![cfg(any(unix, windows))]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration-test helpers outside #[test] functions need the fixture allowances clippy.toml grants only to test functions"
)]

use std::path::Path;

use serde_json::Value;

#[test]
fn bundled_asset_is_seed_san_vrm_1_with_required_features() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/seed-san.vrm");
    let bytes = std::fs::read(&path).expect("read bundled seed-san.vrm");

    assert_eq!(bytes.get(0..4), Some(&b"glTF"[..]));
    assert_eq!(
        read_u32(&bytes, 4),
        2,
        "asset must use the glTF 2 container"
    );
    assert_eq!(
        read_u32(&bytes, 8) as usize,
        bytes.len(),
        "declared container length must match the file"
    );

    let json_len = read_u32(&bytes, 12) as usize;
    assert_eq!(bytes.get(16..20), Some(&b"JSON"[..]));
    let document: Value = serde_json::from_slice(
        bytes
            .get(20..20 + json_len)
            .expect("VRM JSON chunk must be in bounds"),
    )
    .expect("parse VRM JSON chunk");

    let extensions = &document["extensions"];
    assert_eq!(extensions["VRMC_vrm"]["specVersion"], "1.0");
    assert_eq!(extensions["VRMC_springBone"]["specVersion"], "1.0");
    assert!(
        extensions["VRMC_vrm"]["humanoid"]["humanBones"]
            .as_object()
            .is_some_and(|bones| !bones.is_empty())
    );
    assert!(
        extensions["VRMC_vrm"]["expressions"]["preset"]
            .as_object()
            .is_some_and(|expressions| !expressions.is_empty())
    );
    assert!(extensions["VRMC_vrm"]["lookAt"].is_object());
    assert!(
        extensions["VRMC_springBone"]["springs"]
            .as_array()
            .is_some_and(|springs| !springs.is_empty())
    );

    let meta = &extensions["VRMC_vrm"]["meta"];
    assert_eq!(meta["name"], "Seed-san");
    assert_eq!(meta["authors"][0], "VirtualCast, Inc.");
    assert_eq!(meta["creditNotation"], "required");
    assert_eq!(meta["allowRedistribution"].as_bool(), Some(true));
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("four-byte GLB field"),
    )
}
