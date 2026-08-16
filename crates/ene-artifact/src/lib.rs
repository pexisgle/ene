//! # ene-artifact
//!
//! Executable-artifact management: plugins, sidecars, and models are never
//! installed from arbitrary downloads. They are fetched through a **signed
//! catalog** (TUF-inspired metadata), verified against a content-addressable
//! store (SHA-256), and installed with one-generation rollback.
//!
//! Pipeline per artifact:
//!
//! 1. [`CatalogVerifier`] checks the catalog signature, expiry, and
//!    rollback/digest-change rules against the currently installed state.
//! 2. [`ResumableDownload`] fetches the target (`Range` + `ETag`, size-capped)
//!    into a `.part` file, with every redirect re-validated by the caller.
//! 3. [`Cas::put`] verifies size + digest, fsyncs, and atomically activates
//!    the object.
//! 4. [`ArtifactInstaller`] switches the active generation and keeps exactly
//!    one previous generation for rollback; failed updates leave the old
//!    generation active.
//!
//! The crate is deliberately host-side: plugin binaries never touch it.

#![cfg_attr(
    test,
    expect(
        clippy::expect_used,
        reason = "unit tests use expect for concise assertions"
    )
)]

/// Content-addressable storage with verified activation.
pub mod cas;
/// Signed catalog metadata and verification.
pub mod catalog;
pub mod digest;
/// Resumable, size-capped HTTPS downloads.
pub mod download;
pub mod error;
/// Safe extraction of zip payloads (VVPP and friends).
pub mod extract;
/// Install, switch, roll back, and GC artifacts.
pub mod installer;

pub use cas::{Cas, CasEntry};
pub use catalog::{
    ArtifactKind, ArtifactPayload, ArtifactTarget, CatalogMetadata, CatalogVerifier, PayloadFormat,
    SignedCatalog, TrustedCatalogKeys, canonical_catalog_bytes, sign_catalog,
};
pub use digest::{sha256_hex, verify_sha256};
pub use download::{ArtifactProgress, DownloadOutcome, Downloader, InstallStage};
pub use error::{ArtifactError, Result};
pub use installer::{ArtifactInstaller, InstalledArtifact, InstalledState, InstallerConfig};
