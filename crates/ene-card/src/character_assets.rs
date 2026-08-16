//! App-specific asset types and URI resolution for character card `assets`.
//!
//! Every consumer (discovery, import) validates card-declared URIs through
//! [`resolve_asset_uri`], so third-party cards cannot escape the card
//! directory or smuggle an unsupported scheme past a single chokepoint.

use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use url::Url;

use crate::CharacterAsset;
use ene_config::EneConfigError;

/// Default VRM shipped with the app; `ccdefault:` for `x_vrm` resolves here.
pub const DEFAULT_VRM_PATH: &str = "characters/Alicia/AliciaSolid.vrm";
/// Default VRMA motion shipped with the app; `ccdefault:` for `x_vrma`.
pub const DEFAULT_VRMA_PATH: &str = "characters/Alicia/motions/VRMA_01.vrma";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EneAssetKind {
    /// VRM 1.0 model (`x_vrm`).
    Vrm,
    /// VRMA animation (`x_vrma`).
    Vrma,
}

impl CharacterAsset {
    /// Maps the card-declared asset type to a kind Ene consumes.
    ///
    /// `x_vrm` / `x_vrma` are the app-specific types defined by the spec;
    /// the unprefixed forms are tolerated for cards produced by other tools.
    #[must_use]
    pub fn ene_kind(&self) -> Option<EneAssetKind> {
        match self.asset_type.as_str() {
            "x_vrm" | "vrm" => Some(EneAssetKind::Vrm),
            "x_vrma" | "vrma" => Some(EneAssetKind::Vrma),
            _ => None,
        }
    }
}

/// A validated asset URI from a character card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedAssetUri {
    /// `embeded://` — a validated path relative to the card's directory.
    Embedded(PathBuf),
    /// `http://` or `https://` URL (validated, not fetched).
    Remote(Url),
    /// `data:` URL with a validated header; the payload is still encoded.
    Data {
        /// Media type before the parameters (may be empty).
        media_type: Option<String>,
        /// Whether the payload is base64 (`;base64` parameter present).
        is_base64: bool,
        /// Encoded payload after the comma.
        payload: String,
    },
    /// `ccdefault:` — the application default for the asset type.
    AppDefault,
}

/// Resolves and validates a card-declared `assets[].uri`.
///
/// Supported schemes are `embeded://` (validated relative path), `http(s)://`
/// (parsed with `url`), `data:` (structure-validated), and `ccdefault:`. A
/// value without a scheme is treated as an embedded relative path for
/// tolerance with non-spec producers. Anything else is rejected with
/// [`EneConfigError::UnsupportedAssetUriScheme`].
///
/// # Errors
///
/// - [`EneConfigError::UnsafeAssetPath`] for traversal or absolute paths;
/// - [`EneConfigError::InvalidAssetUri`] for malformed URIs;
/// - [`EneConfigError::UnsupportedAssetUriScheme`] for unknown schemes.
pub fn resolve_asset_uri(uri: &str) -> Result<ResolvedAssetUri, EneConfigError> {
    if let Some(path) = strip_scheme(uri, "embeded://").or_else(|| strip_scheme(uri, "embedded://"))
    {
        return validate_embedded_path(path).map(ResolvedAssetUri::Embedded);
    }
    if strip_scheme(uri, "http://").is_some() || strip_scheme(uri, "https://").is_some() {
        return parse_http_url(uri).map(ResolvedAssetUri::Remote);
    }
    if strip_scheme(uri, "data:").is_some() {
        return parse_data_uri(uri);
    }
    if uri.eq_ignore_ascii_case("ccdefault:") {
        return Ok(ResolvedAssetUri::AppDefault);
    }
    if let Some((scheme, _)) = uri
        .split(['/', '\\'])
        .next()
        .and_then(|head| head.split_once(':'))
    {
        return Err(EneConfigError::UnsupportedAssetUriScheme(
            scheme.to_string(),
        ));
    }
    validate_embedded_path(uri).map(ResolvedAssetUri::Embedded)
}

/// Decodes a `data:` payload produced by [`resolve_asset_uri`].
///
/// Base64 payloads use the standard alphabet (padded first, unpadded as a
/// fallback); non-base64 payloads are percent-decoded text. Decoding stops
/// once `max_bytes` is exceeded so a card cannot force a large allocation at
/// import time.
///
/// # Errors
///
/// - [`EneConfigError::InvalidAssetUri`] for malformed base64 or
///   percent-encoding;
/// - [`EneConfigError::AssetPayloadTooLarge`] when the decoded size exceeds
///   `max_bytes`.
pub fn decode_data_payload(
    is_base64: bool,
    payload: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, EneConfigError> {
    if is_base64 {
        let compact: String = payload
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect();
        // Base64 of `n` bytes is at most `n * 4 / 3 + 4` characters, so the
        // encoded length bounds the decoded allocation before decoding.
        if compact.len() as u64 > max_bytes.saturating_mul(4).div_ceil(3).saturating_add(4) {
            return Err(EneConfigError::AssetPayloadTooLarge(max_bytes));
        }
        let decoded = STANDARD
            .decode(compact.as_bytes())
            .or_else(|_| STANDARD_NO_PAD.decode(compact.as_bytes()))
            .map_err(|_| EneConfigError::InvalidAssetUri(payload.to_string()))?;
        if decoded.len() as u64 > max_bytes {
            return Err(EneConfigError::AssetPayloadTooLarge(max_bytes));
        }
        Ok(decoded)
    } else {
        percent_decode(payload, max_bytes)
    }
}

/// Returns `uri` without the `scheme` prefix, case-insensitively.
fn strip_scheme<'a>(uri: &'a str, scheme: &str) -> Option<&'a str> {
    uri.get(..scheme.len())
        .filter(|prefix| prefix.eq_ignore_ascii_case(scheme))
        .map(|_| &uri[scheme.len()..])
}

/// Validates an embedded path: relative, `/`-separated, no traversal, no
/// drive prefixes, no percent-encoding (which could smuggle `..` after
/// decoding on a case-insensitive filesystem).
fn validate_embedded_path(path: &str) -> Result<PathBuf, EneConfigError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains(['\\', '\0', '%'])
    {
        return Err(EneConfigError::InvalidAssetUri(path.to_string()));
    }
    let mut out = PathBuf::new();
    for component in path.split('/') {
        if component == ".." || is_drive_prefix(component) {
            return Err(EneConfigError::UnsafeAssetPath(path.to_string()));
        }
        if component.is_empty() || component == "." {
            return Err(EneConfigError::InvalidAssetUri(path.to_string()));
        }
        out.push(component);
    }
    Ok(out)
}

/// `true` for a Windows drive-relative prefix such as `C:model.vrm`.
fn is_drive_prefix(component: &str) -> bool {
    component.len() >= 2
        && component.as_bytes()[0].is_ascii_alphabetic()
        && component.as_bytes()[1] == b':'
}

fn parse_http_url(uri: &str) -> Result<Url, EneConfigError> {
    let url = Url::parse(uri).map_err(|_| EneConfigError::InvalidAssetUri(uri.to_string()))?;
    if url.host_str().is_none() {
        return Err(EneConfigError::InvalidAssetUri(uri.to_string()));
    }
    Ok(url)
}

fn parse_data_uri(uri: &str) -> Result<ResolvedAssetUri, EneConfigError> {
    let colon = uri
        .find(':')
        .ok_or_else(|| EneConfigError::InvalidAssetUri(uri.to_string()))?;
    let comma = uri[colon + 1..]
        .find(',')
        .map(|offset| colon + 1 + offset)
        .ok_or_else(|| EneConfigError::InvalidAssetUri(uri.to_string()))?;
    let payload = &uri[comma + 1..];
    if payload.is_empty() {
        return Err(EneConfigError::InvalidAssetUri(uri.to_string()));
    }
    let mut parts = uri[colon + 1..comma].split(';');
    let media_type = parts.next().filter(|t| !t.is_empty()).map(str::to_string);
    let is_base64 = parts.any(|t| t.eq_ignore_ascii_case("base64"));
    Ok(ResolvedAssetUri::Data {
        media_type,
        is_base64,
        payload: payload.to_string(),
    })
}

fn percent_decode(payload: &str, max_bytes: u64) -> Result<Vec<u8>, EneConfigError> {
    let bytes = payload.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(hi) = bytes.get(index + 1).copied().and_then(hex_value) else {
                return Err(EneConfigError::InvalidAssetUri(payload.to_string()));
            };
            let Some(lo) = bytes.get(index + 2).copied().and_then(hex_value) else {
                return Err(EneConfigError::InvalidAssetUri(payload.to_string()));
            };
            out.push((hi << 4) | lo);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
        if out.len() as u64 > max_bytes {
            return Err(EneConfigError::AssetPayloadTooLarge(max_bytes));
        }
    }
    Ok(out)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_paths_resolve_relative() {
        let uri = resolve_asset_uri("embeded://assets/x_vrm/3d/model.vrm")
            .expect("embedded uri resolves");
        assert_eq!(
            uri,
            ResolvedAssetUri::Embedded(PathBuf::from("assets/x_vrm/3d/model.vrm"))
        );
    }

    #[test]
    fn misspelled_embedded_scheme_is_tolerated() {
        let uri = resolve_asset_uri("embedded://model.vrm").expect("misspelled scheme resolves");
        assert_eq!(uri, ResolvedAssetUri::Embedded(PathBuf::from("model.vrm")));
    }

    #[test]
    fn bare_relative_uri_is_treated_as_embedded() {
        let uri = resolve_asset_uri("model.vrm").expect("bare relative path resolves");
        assert_eq!(uri, ResolvedAssetUri::Embedded(PathBuf::from("model.vrm")));
    }

    #[test]
    fn traversal_paths_are_rejected() {
        for uri in [
            "embeded://../evil.vrm",
            "embeded://a/../../evil.vrm",
            "embeded://..",
        ] {
            let err = resolve_asset_uri(uri).expect_err("traversal must fail");
            assert!(
                matches!(err, EneConfigError::UnsafeAssetPath(_)),
                "expected UnsafeAssetPath for {uri:?}, got {err}"
            );
        }
    }

    #[test]
    fn absolute_and_drive_paths_are_rejected() {
        for uri in [
            "embeded:///etc/passwd",
            "embeded://C:model.vrm",
            "embeded://a/C:model.vrm",
            "embeded://C:\\model.vrm",
            "embeded://a\\b.vrm",
            "embeded://",
            "embeded://a//b.vrm",
            "embeded://a/./b.vrm",
            "embeded://a%2Fb.vrm",
            "..%2Fevil.vrm",
        ] {
            let err = resolve_asset_uri(uri).expect_err("unsafe path must fail");
            assert!(
                matches!(
                    err,
                    EneConfigError::UnsafeAssetPath(_) | EneConfigError::InvalidAssetUri(_)
                ),
                "expected path rejection for {uri:?}, got {err}"
            );
        }
    }

    #[test]
    fn http_and_https_uris_parse() {
        let uri = resolve_asset_uri("https://example.com/model.vrm").expect("https resolves");
        assert!(
            matches!(&uri, ResolvedAssetUri::Remote(url) if url.as_str() == "https://example.com/model.vrm"),
            "expected remote URL, got {uri:?}"
        );

        let uri = resolve_asset_uri("http://example.com/model.vrm").expect("http resolves");
        assert!(matches!(uri, ResolvedAssetUri::Remote(_)));
    }

    #[test]
    fn malformed_http_uris_are_rejected() {
        for uri in ["https://", "https://exa mple.com"] {
            let err = resolve_asset_uri(uri).expect_err("malformed http must fail");
            assert!(
                matches!(err, EneConfigError::InvalidAssetUri(_)),
                "expected InvalidAssetUri for {uri:?}, got {err}"
            );
        }
    }

    #[test]
    fn unknown_schemes_are_reported() {
        for uri in [
            "file:///etc/passwd",
            "ftp://host/model.vrm",
            "__asset:icon.png",
        ] {
            let err = resolve_asset_uri(uri).expect_err("unknown scheme must fail");
            assert!(
                matches!(err, EneConfigError::UnsupportedAssetUriScheme(_)),
                "expected UnsupportedAssetUriScheme for {uri:?}, got {err}"
            );
        }
    }

    #[test]
    fn data_uris_parse_headers() {
        let uri = resolve_asset_uri("data:model/vrm;base64,QUJD").expect("base64 data uri");
        assert_eq!(
            uri,
            ResolvedAssetUri::Data {
                media_type: Some("model/vrm".to_string()),
                is_base64: true,
                payload: "QUJD".to_string(),
            }
        );

        let uri = resolve_asset_uri("data:text/plain,hello%20world").expect("text data uri");
        assert_eq!(
            uri,
            ResolvedAssetUri::Data {
                media_type: Some("text/plain".to_string()),
                is_base64: false,
                payload: "hello%20world".to_string(),
            }
        );

        for uri in ["data:,", "data:base64,"] {
            let err = resolve_asset_uri(uri).expect_err("malformed data uri must fail");
            assert!(
                matches!(err, EneConfigError::InvalidAssetUri(_)),
                "expected InvalidAssetUri for {uri:?}, got {err}"
            );
        }
    }

    #[test]
    fn ccdefault_is_exact_and_case_insensitive() {
        assert_eq!(
            resolve_asset_uri("ccdefault:").expect("ccdefault resolves"),
            ResolvedAssetUri::AppDefault
        );
        assert_eq!(
            resolve_asset_uri("CCDEFAULT:").expect("case-insensitive ccdefault resolves"),
            ResolvedAssetUri::AppDefault
        );
        assert!(matches!(
            resolve_asset_uri("ccdefault://x"),
            Err(EneConfigError::UnsupportedAssetUriScheme(_))
        ));
    }

    #[test]
    fn empty_uri_is_invalid() {
        assert!(matches!(
            resolve_asset_uri(""),
            Err(EneConfigError::InvalidAssetUri(_))
        ));
    }

    #[test]
    fn data_payloads_decode_with_caps() {
        let decoded = decode_data_payload(true, "QUJD", 1024).expect("base64 decodes");
        assert_eq!(decoded, b"ABC");

        let decoded = decode_data_payload(false, "hello%20world", 1024).expect("percent decodes");
        assert_eq!(decoded, b"hello world");

        let err = decode_data_payload(true, "QUJD", 2).expect_err("cap enforced");
        assert!(matches!(err, EneConfigError::AssetPayloadTooLarge(_)));

        let err = decode_data_payload(false, "hello%zz", 1024).expect_err("bad percent");
        assert!(matches!(err, EneConfigError::InvalidAssetUri(_)));

        let err = decode_data_payload(true, "!!!", 1024).expect_err("bad base64");
        assert!(matches!(err, EneConfigError::InvalidAssetUri(_)));
    }

    #[test]
    fn asset_kinds_map_x_types_and_tolerate_unprefixed() {
        use crate::CharacterAsset;

        let asset = |asset_type: &str| CharacterAsset {
            asset_type: asset_type.to_string(),
            uri: String::new(),
            name: String::new(),
            ext: String::new(),
            extra: indexmap::IndexMap::new(),
        };
        assert_eq!(asset("x_vrm").ene_kind(), Some(EneAssetKind::Vrm));
        assert_eq!(asset("vrm").ene_kind(), Some(EneAssetKind::Vrm));
        assert_eq!(asset("x_vrma").ene_kind(), Some(EneAssetKind::Vrma));
        assert_eq!(asset("vrma").ene_kind(), Some(EneAssetKind::Vrma));
        assert_eq!(asset("icon").ene_kind(), None);
        assert_eq!(asset("").ene_kind(), None);
    }
}
