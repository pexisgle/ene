use crate::AssetVersionView;

#[test]
fn asset_version_view_variant_fields_roundtrip() {
    let view = AssetVersionView {
        version: "b4282/avx2".to_owned(),
        size_bytes: Some(42),
        recommended: true,
        installed: false,
        variant_id: "avx2".to_owned(),
        label: "AVX2".to_owned(),
        backend: "avx2".to_owned(),
        release_tag: "b4282".to_owned(),
    };
    let raw = serde_json::to_string(&view).expect("encode");
    let decoded: AssetVersionView = serde_json::from_str(&raw).expect("decode");
    assert_eq!(decoded, view);
}
