//! Capability identity and version types for plugin `provides` / `requires`
//! declarations.
//!
//! This module is the authority for *what a capability is called and how its
//! version is expressed* — the `name@version` contract that plugins declare
//! via `x-ene-capabilities` and that the host resolves at startup. Parsing of
//! the schema block itself lives in [`crate::declaration`]; these types carry
//! the validated results.
//!
//! Capability *versions* are full semver versions (`1` = `1.0.0`). A major
//! bump is a compatibility break for the capability's contract — the same
//! discipline as the wire ABI — because callers express dependencies as
//! `VersionReq` ranges over it. Pre-release and build-metadata forms are
//! banned at parse time (see [`crate::declaration::CapabilityRejection`]):
//! pre-release spelling collides with the `?` soft-requirement marker, and
//! build metadata carries no precedence meaning.

use crate::error::ConnectorError;
use crate::identity::valid_id_char;
use std::fmt;
use std::str::FromStr;

/// A stable, optionally namespaced identifier for a plugin capability.
///
/// The form is either a plain name (`gguf-runner`) or one slash-namespaced
/// pair (`tts/synthesize`, `g2p/ja`); at most one `/` is allowed. Each
/// segment uses the same charset as [`CredentialId`](crate::CredentialId)
/// (`[A-Za-z0-9._-]`, no leading or trailing `.`). The `@` that separates
/// name from version in a declaration is deliberately not a name character,
/// so the name/version split is unambiguous.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct CapabilityId(String);

impl CapabilityId {
    /// Returns `true` when `id` is non-empty, contains no `@`, uses only
    /// accepted characters in each of at most two `/`-separated segments,
    /// and has no segment starting or ending with `.`.
    #[must_use]
    pub fn is_valid(id: &str) -> bool {
        if id.is_empty() || id.contains('@') {
            return false;
        }
        let mut segments = id.split('/');
        let Some(first) = segments.next() else {
            return false;
        };
        if !valid_segment(first) {
            return false;
        }
        match segments.next() {
            Some(second) => valid_segment(second) && segments.next().is_none(),
            None => true,
        }
    }

    /// Creates a capability id, validating the charset and segment form.
    ///
    /// # Errors
    /// Returns [`ConnectorError::Internal`] when `id` is empty, contains `@`,
    /// has more than one `/`, or uses characters outside `[A-Za-z0-9._-]`.
    pub fn try_new(id: impl Into<String>) -> Result<Self, ConnectorError> {
        let s = id.into();
        if Self::is_valid(&s) {
            Ok(Self(s))
        } else {
            Err(ConnectorError::internal(format!(
                "invalid capability ID: '{s}'"
            )))
        }
    }

    /// Returns the full identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for CapabilityId {
    type Err = ConnectorError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_new(s)
    }
}

/// Returns `true` when `segment` is non-empty, does not start or end with
/// `.`, and uses only `[A-Za-z0-9._-]` characters.
fn valid_segment(segment: &str) -> bool {
    !segment.is_empty()
        && !segment.starts_with('.')
        && !segment.ends_with('.')
        && segment.as_bytes().iter().all(|&b| valid_id_char(b))
}

/// A capability a plugin provides, with its concrete semver version.
///
/// [`Debug`] is implemented manually because `semver::Version` deliberately
/// derives no `Debug`; the version renders via its canonical string form.
#[derive(Clone, PartialEq, Eq)]
pub struct ProvidedCapability {
    /// The capability's stable name (`tts/synthesize`, `g2p/ja`, …).
    pub name: CapabilityId,
    /// The exact version provided. `1` in a declaration becomes `1.0.0`.
    pub version: semver::Version,
}

impl fmt::Debug for ProvidedCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProvidedCapability")
            .field("name", &self.name)
            .field("version", &self.version.to_string())
            .finish()
    }
}

/// A capability a plugin requires, expressed as a semver range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredCapability {
    /// The capability's stable name.
    pub name: CapabilityId,
    /// The accepted version range (`^1`, `1`, `>=1.2.0, <2`, …).
    pub req: semver::VersionReq,
    /// When `true`, an unmet requirement does not block startup — the
    /// plugin is expected to fall back to a built-in implementation and may
    /// retry the dependency at runtime. The declaration spells this as a
    /// trailing `?` (`g2p/ja@^1?`).
    pub soft: bool,
}

impl fmt::Display for ProvidedCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

impl fmt::Display for RequiredCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.req)?;
        if self.soft {
            f.write_str("?")?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "tests use unwrap for concise failure messages"
)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_and_slash_namespaced_ids() {
        for valid in [
            "gguf-runner",
            "onnx-runner",
            "tts/synthesize",
            "g2p/ja",
            "a.b/c-d_e",
        ] {
            assert!(CapabilityId::is_valid(valid), "{valid} must be valid");
        }
    }

    #[test]
    fn rejects_empty_at_and_bad_chars() {
        assert!(!CapabilityId::is_valid(""));
        assert!(!CapabilityId::is_valid("a@1"));
        assert!(!CapabilityId::is_valid("has space"));
        assert!(!CapabilityId::is_valid("semi;colon"));
        assert!(!CapabilityId::is_valid("a/b/c"));
        assert!(!CapabilityId::is_valid("/leading"));
        assert!(!CapabilityId::is_valid("trailing/"));
        assert!(!CapabilityId::is_valid(".leading"));
        assert!(!CapabilityId::is_valid("trailing."));
        assert!(CapabilityId::try_new("bad@name").is_err());
    }

    #[test]
    fn round_trips_via_display_and_fromstr() {
        let id = CapabilityId::try_new("g2p/ja").unwrap();
        assert_eq!(id.as_str(), "g2p/ja");
        assert_eq!(format!("{id}"), "g2p/ja");
        assert_eq!(id.to_string().parse::<CapabilityId>().unwrap(), id);
    }

    #[test]
    fn capability_display_round_trips() {
        let provided = ProvidedCapability {
            name: CapabilityId::try_new("tts/synthesize").unwrap(),
            version: "1.0.0".parse().unwrap(),
        };
        assert_eq!(provided.to_string(), "tts/synthesize@1.0.0");
        let required = RequiredCapability {
            name: CapabilityId::try_new("g2p/ja").unwrap(),
            req: "^1".parse().unwrap(),
            soft: true,
        };
        assert_eq!(required.to_string(), "g2p/ja@^1?");
    }
}
