//! Credential registry contracts: non-secret refs, secret hygiene, and the
//! request-builder store pattern.
//!
//! [`CredentialRef`] is the only credential value that may leave this crate
//! freely: it names a credential without carrying any secret material. Key
//! material lives solely in [`SecretValue`], which has no [`core::fmt::Debug`]
//! implementation and is zeroized on drop.
//!
//! Secrets enter only through the Host-local protected path (a future
//! behaviors-stage store): [`RegisterCredentialCommand`] deliberately carries
//! no secret field, so registration can never smuggle key material through
//! the registry. [`CredentialStore::with_bearer`] exposes the bearer only
//! inside a caller closure; the caller must build an owned request there and
//! send it after the closure returns, because the borrowed bearer never
//! escapes the closure's lifetime.

use std::collections::HashMap;
use std::sync::Mutex;

use ene_primitive::{RawId, WallClockWithTz};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Non-secret handle naming one stored credential.
///
/// Safe to clone, log, and persist: it carries provider and label only, never
/// key material.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CredentialRef {
    /// Stable composite key of `provider:label`, not a secret.
    pub id: String,
    /// Provider name; matched exactly.
    pub provider: String,
    /// Owner-chosen label distinguishing credentials of one provider.
    pub label: String,
}

/// Command registering a credential ref in the registry.
///
/// There is intentionally no secret field: the bearer is provisioned through
/// the Host-local protected path directly into the [`CredentialStore`], never
/// through this command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterCredentialCommand {
    /// Provider the credential belongs to; must be non-blank.
    pub provider: String,
    /// Owner-chosen label for the credential.
    pub label: String,
}

/// Outcome of a registry `register` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterOutcome {
    /// A new ref was persisted.
    Registered(CredentialRef),
    /// The provider name was blank.
    InvalidProvider,
    /// A ref already exists; the stored ref was left untouched (no overwrite).
    AlreadyExists(CredentialRef),
}

/// Availability of one credential across registry and store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialAvailability {
    /// True only when the ref is known to the registry and the store holds
    /// its bearer.
    pub present: bool,
    /// The known ref, if the registry knows it.
    pub credential: Option<CredentialRef>,
}

/// Host-facing notification about a credential state change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialNotify {
    /// The bearer no longer works; the owner must reauthenticate.
    NeedsReauthentication(CredentialRef),
    /// The credential was revoked and its ref removed.
    Revoked(CredentialRef),
}

/// Secret key material, confined to this crate.
///
/// The bytes are `pub(crate)` so only in-crate store implementations can
/// touch them. There is deliberately no [`core::fmt::Debug`] implementation:
/// deriving or hand-writing one would risk logging bearer material. Memory is
/// zeroized on drop via [`ZeroizeOnDrop`].
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SecretValue {
    /// Raw bearer bytes; never logged, never rendered.
    pub(crate) bytes: Vec<u8>,
}

impl SecretValue {
    /// Wraps raw bearer bytes for an in-crate store.
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    /// Borrows the bearer bytes for an in-crate store.
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Technical failures of credential storage.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CredentialTechnicalError {
    /// The credential store was unreachable or rejected the operation.
    #[error("credential storage unavailable: {reason}")]
    StorageUnavailable {
        /// Backend-supplied cause, without secret material.
        reason: String,
    },
}

/// Persistence boundary for non-secret credential refs.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CredentialRefRepository: Send + Sync {
    /// Persists a credential ref.
    async fn save_ref(&self, cred: CredentialRef) -> Result<(), CredentialTechnicalError>;

    /// Loads the ref for one `(provider, label)` pair, if any.
    async fn load_ref(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<Option<CredentialRef>, CredentialTechnicalError>;

    /// Lists all known refs.
    async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError>;
}

/// Bearer store with a request-builder access pattern.
///
/// Implementations hold [`SecretValue`] internally and expose the bearer only
/// as a `&str` borrowed into the caller's closure `f`. The caller must build
/// an owned request (headers, body) inside the closure and perform I/O after
/// it returns: the borrow cannot escape, so there is deliberately no getter
/// returning an owned secret. Existence checks via [`CredentialStore::contains`]
/// are non-secret and safe to branch on.
pub trait CredentialStore: Send + Sync {
    /// Runs `f` with the bearer for `cred`.
    ///
    /// Errors when the credential is unknown, the secret is not valid UTF-8,
    /// or the backend fails; the error never carries secret material.
    fn with_bearer<R>(
        &self,
        cred: &CredentialRef,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError>;

    /// Deletes the bearer for `cred`.
    fn delete(&self, cred: &CredentialRef) -> Result<(), CredentialTechnicalError>;

    /// Reports whether the store holds a bearer for `cred`.
    ///
    /// Existence is non-secret metadata.
    fn contains(&self, cred: &CredentialRef) -> bool;
}

/// Registers a credential ref, never overwriting an existing one.
///
/// A blank provider (empty or whitespace-only) yields
/// [`RegisterOutcome::InvalidProvider`] without touching the repository. When
/// a ref already exists for `(provider, label)`, the stored ref is returned
/// in [`RegisterOutcome::AlreadyExists`] and no write occurs.
pub async fn register(
    cmd: RegisterCredentialCommand,
    repo: &impl CredentialRefRepository,
) -> Result<RegisterOutcome, CredentialTechnicalError> {
    if cmd.provider.trim().is_empty() {
        return Ok(RegisterOutcome::InvalidProvider);
    }
    if let Some(existing) = repo.load_ref(&cmd.provider, &cmd.label).await? {
        return Ok(RegisterOutcome::AlreadyExists(existing));
    }
    let cred = CredentialRef {
        id: format!("{}:{}", cmd.provider, cmd.label),
        provider: cmd.provider,
        label: cmd.label,
    };
    repo.save_ref(cred.clone()).await?;
    Ok(RegisterOutcome::Registered(cred))
}

/// Combines registry knowledge with store presence into one availability fact.
///
/// `repo_known` reports whether the registry holds the ref; store presence is
/// read via [`CredentialStore::contains`]. The credential is available only
/// when both agree.
pub fn credential_availability(
    cred: &CredentialRef,
    repo_known: bool,
    store: &impl CredentialStore,
) -> CredentialAvailability {
    let present = repo_known && store.contains(cred);
    CredentialAvailability {
        present,
        credential: repo_known.then(|| cred.clone()),
    }
}

/// In-memory bearer store for tests and local development only.
///
/// Holds [`SecretValue`] entries keyed by `(provider, label)` behind a
/// mutex. Not a production backend: contents live in process memory and
/// vanish on restart.
///
/// [`core::fmt::Debug`] lists only the public refs and the entry count, never
/// secret material.
pub struct MemoryCredentialStore {
    /// Entries keyed by `(provider, label)`; values pair the public ref with
    /// its confined secret.
    entries: Mutex<HashMap<(String, String), (CredentialRef, SecretValue)>>,
}

impl core::fmt::Debug for MemoryCredentialStore {
    /// Renders the entry count and public refs; secrets are never rendered.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let refs: Vec<&CredentialRef> = entries.values().map(|(cred, _)| cred).collect();
        f.debug_struct("MemoryCredentialStore")
            .field("len", &refs.len())
            .field("refs", &refs)
            .finish()
    }
}

impl Default for MemoryCredentialStore {
    /// Creates an empty store holding no bearers.
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryCredentialStore {
    /// Creates an empty store holding no bearers.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Inserts or replaces the bearer for `cred`.
    ///
    /// Test/dev provisioning path standing in for the Host-local protected
    /// path; production backends must not accept secrets this casually.
    pub fn insert(&self, cred: CredentialRef, secret: &str) {
        let key = (cred.provider.clone(), cred.label.clone());
        let mut entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.insert(key, (cred, SecretValue::new(secret.as_bytes().to_vec())));
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn with_bearer<R>(
        &self,
        cred: &CredentialRef,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        let entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some((_, secret)) = entries.get(&(cred.provider.clone(), cred.label.clone())) else {
            let id = cred.id.as_str();
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("unknown credential {id}"),
            });
        };
        let Ok(bearer) = core::str::from_utf8(secret.bytes()) else {
            let id = cred.id.as_str();
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("stored secret for {id} is not valid UTF-8"),
            });
        };
        Ok(f(bearer))
    }

    fn delete(&self, cred: &CredentialRef) -> Result<(), CredentialTechnicalError> {
        let mut entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.remove(&(cred.provider.clone(), cred.label.clone()));
        Ok(())
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        let entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.contains_key(&(cred.provider.clone(), cred.label.clone()))
    }
}

/// Opaque device identity for device pairing (Group K, Stage 2 thin scope).
///
/// Wraps a [`RawId`] with no `From` implementations to or from any other
/// type: a `DeviceId` names one logical device in this crate's pairing
/// records only. It is distinct from the wire `DeviceWireId` carried in
/// `ene-api` envelopes: mapping between wire and domain identities happens in
/// Host composition at the call boundary, never in this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId(
    /// Wrapped opaque identity; meaningful only as a device name in this crate.
    pub RawId,
);

/// One paired device: its minted identity, display string, and pairing time.
///
/// `descriptor` is an owner-supplied display string (for example `"phone"`);
/// it carries no secret material, so derived [`core::fmt::Debug`] is safe.
/// `paired_at` records when pairing completed, for display and audit only.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceRecord {
    /// Device identity minted at approval; never reused.
    pub id: DeviceId,
    /// Owner-visible display string naming the device.
    pub descriptor: String,
    /// Wall-clock time with its creation offset recording when pairing completed.
    pub paired_at: WallClockWithTz,
}

/// One requested-but-not-yet-approved pairing.
///
/// `descriptor` is the owner-supplied display string from the request; it
/// carries no secret material, so derived [`core::fmt::Debug`] is safe.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PendingPairing {
    /// Owner-visible display string naming the requesting device.
    pub descriptor: String,
    /// Wall-clock time with its creation offset recording when requested.
    pub requested_at: WallClockWithTz,
}

/// Outcome of a pairing request.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DevicePairingStatus {
    /// The descriptor is already paired; carries the existing record.
    Paired {
        /// Existing paired-device record, unchanged.
        device: DeviceRecord,
    },
    /// The descriptor is not yet paired; carries the pending request.
    Pending {
        /// Pending request, newly recorded or previously stored.
        pending: PendingPairing,
    },
}

/// Persistence boundary for device pairing requests and approvals.
///
/// Revocation is explicitly deferred: Stage 2 thin scope provides no
/// remove/revoke method, so paired records only accumulate.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait DevicePairingRepository: Send + Sync {
    /// Records a pairing request for `descriptor`.
    ///
    /// Returns [`DevicePairingStatus::Paired`] only when `descriptor` is
    /// already paired (idempotent re-request leaves the stored record
    /// untouched). Otherwise records a pending request — or returns the
    /// existing pending entry when one is already stored — and returns
    /// [`DevicePairingStatus::Pending`].
    ///
    /// Blank-descriptor contract: Host ingress validates that the descriptor
    /// is non-blank before calling. Implementations perform no blank check
    /// themselves: a blank `descriptor` is recorded as pending like any other
    /// string, never rejected here, so callers must not rely on this method
    /// to catch blank input.
    async fn request_pairing(
        &self,
        descriptor: String,
    ) -> Result<DevicePairingStatus, CredentialTechnicalError>;

    /// Approves the pending request for `descriptor`, pairing the device and
    /// issuing its one-time pairing secret.
    ///
    /// On a known pending descriptor this mints a fresh device identity via
    /// [`RawId::new`] and a fresh pairing secret (a second [`RawId::new`]
    /// rendered as UUID text), moves the entry from pending to paired, stores
    /// the record (id, descriptor, and timestamps only — never the secret),
    /// and returns the record together with the secret. An unknown descriptor
    /// yields `Ok(None)` — not an error; the caller maps that outcome to a
    /// clarification request. Re-approving an already-paired descriptor
    /// returns the existing record unchanged (no fresh device identity) with
    /// a freshly minted secret, rotating the previous one.
    ///
    /// Secret custody flow: the trait is secret-free in storage. The approve
    /// caller (Host composition) holds the returned secret in memory,
    /// short-lived, transfers it once over a trusted inlet (approve-time
    /// one-time display / protected client file), and provisions it into its
    /// runtime secret map for later ownership-proof verification. The secret
    /// is never logged, never rendered in `Debug`, and never travels the
    /// wire — only the pairing proof derived from it leaves the device.
    ///
    /// Approval records an Owner decision transported from a trusted inlet;
    /// the repository never decides whether pairing is allowed, it records
    /// the decision it was given.
    async fn approve_pending(
        &self,
        descriptor: &str,
    ) -> Result<Option<(DeviceRecord, String)>, CredentialTechnicalError>;

    /// Loads the paired record for `id`, if any.
    async fn find_device(
        &self,
        id: &DeviceId,
    ) -> Result<Option<DeviceRecord>, CredentialTechnicalError>;

    /// Lists all currently pending pairing requests.
    async fn list_pending(&self) -> Result<Vec<PendingPairing>, CredentialTechnicalError>;
}

/// Pairing ownership proof (Group K device-auth): HMAC-SHA256 over a
/// single-use nonce, keyed by the pairing secret.
///
/// The secret travels a trusted inlet only: it is shown once at approve time
/// for one-time display, or written to a protected client file. It is never
/// logged, never rendered in `Debug`, and never sent over the wire — only
/// the proof hex leaves the device. The nonce is single-use by caller
/// contract: the Host mints a fresh nonce per challenge and rejects reuse,
/// so a captured proof cannot be replayed.
///
/// ```
/// use ene_credential::{pairing_proof_hex, verify_pairing_proof};
///
/// let proof = pairing_proof_hex("pairing-secret", "one-time-nonce");
/// assert!(verify_pairing_proof("pairing-secret", "one-time-nonce", &proof));
/// assert!(!verify_pairing_proof("other-secret", "one-time-nonce", &proof));
/// ```
#[must_use]
pub fn pairing_proof_hex(secret: &str, nonce: &str) -> String {
    encode_hex_lower(&compute_pairing_mac(secret, nonce))
}

/// Verifies a pairing ownership proof against the secret and nonce.
///
/// Hex-decodes `proof` (malformed input yields `false`) and compares the
/// bytes against the recomputed MAC in constant time via `subtle`, so no
/// early exit leaks how much of the proof matched. See
/// [`pairing_proof_hex`] for the secret-inlet and nonce-reuse caller
/// contracts, which apply here unchanged.
#[must_use]
pub fn verify_pairing_proof(secret: &str, nonce: &str, proof: &str) -> bool {
    let Some(decoded) = decode_hex_lower(proof) else {
        return false;
    };
    let recomputed = compute_pairing_mac(secret, nonce);
    bool::from(recomputed.as_slice().ct_eq(decoded.as_slice()))
}

/// Computes the raw HMAC-SHA256 of `nonce` keyed by `secret`.
///
/// HMAC accepts keys of any length, so construction cannot fail in practice;
/// on the impossible error this yields zeros rather than panicking.
fn compute_pairing_mac(secret: &str, nonce: &str) -> [u8; 32] {
    let mut out = [0_u8; 32];
    if let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) {
        mac.update(nonce.as_bytes());
        let digest = mac.finalize().into_bytes();
        out.copy_from_slice(&digest);
    }
    out
}

/// Renders bytes as lowercase hex, two characters per byte.
fn encode_hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

/// Decodes lowercase hex; yields [`None`] for odd lengths or any byte
/// outside `0-9`/`a-f` (uppercase included, so only canonical proofs parse).
fn decode_hex_lower(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let (pairs, _) = bytes.as_chunks::<2>();
    let mut out = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let hi = hex_val(pair[0])?;
        let lo = hex_val(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

/// Value of one lowercase hex digit, or [`None`] for any other byte.
fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

use std::collections::BTreeMap;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// File-backed custody for device-auth verification material.
///
/// Pairing secrets minted at approval must survive both process boundaries
/// (a separate `approve-device` process persists them while the serving
/// process verifies proofs) and Host restarts, so verification material
/// cannot live only in the serving process's memory. This store keeps one
/// entry per paired device in a protected file shared across processes and
/// restarts. Reads are read-through on every call: nothing is cached, so a
/// verifier always observes the latest persisted rotation or revocation.
/// Authentication stays per-connection-once, so the extra file read costs
/// correctness nothing it cannot afford.
///
/// The whole file is one JSON document mapping canonical device UUID text to
/// an entry holding the secret (lowercase hex), the owner-visible
/// descriptor, and the entry write time as RFC 3339:
///
/// ```json
/// {"devices": {"123e4567-e89b-12d3-a456-426614174000": {"secret_hex": "00ab",
/// "descriptor": "phone", "paired_at": "2026-09-08T12:00:00+09:00"}}}
/// ```
///
/// Secret custody: generation stays with the caller (the pairing repository
/// approve path mints the secret); this store only persists and returns
/// custody. [`load_secret`](FileDeviceAuthStore::load_secret) hands back an
/// owned [`SecretValue`], which is zeroized on drop and has no `Debug`
/// rendering. Secrets and descriptors are never logged and never appear in
/// this type's `Debug` output, which shows the path and the entry count
/// only.
///
/// File protection: on Unix the file at rest must be mode `0600`. Opening an
/// existing file with any other mode attempts to tighten it to `0600` and
/// fails when tightening does not stick; newly written files (including the
/// staging temp) are created `0600`. On non-Unix platforms there is no mode
/// check: the OS-specific protection story is documented at the call site
/// instead, and the file must still live in a directory only the owner can
/// read.
///
/// Caller-owned directory: the caller creates the parent directory. Opening
/// fails when the parent directory is missing, so a misconfigured data
/// directory can never silently redirect the store. A missing file is not an
/// error: opening succeeds empty and the file is created lazily on the first
/// save. A malformed file is always an error, never a silent default.
///
/// Atomicity story: every mutation rewrites the whole file by staging the
/// new bytes to a temp file in the same directory (created `0600` on Unix,
/// flushed with `sync_all`) and renaming it over the target. The rename is
/// the atomic replace: concurrent readers observe the old or the new
/// document whole, so torn reads are impossible. Read-modify-write cycles
/// still race across processes: concurrent approves of different devices are
/// last-writer-wins and can drop an entry, and concurrent approves of one
/// device are a rotation race. Approval is therefore an owner-serialized
/// operation; this store provides durability, not mutual exclusion.
///
/// Backup-exclusion contract: this file holds Group K verification material
/// with E classification. It must never enter backups or exports and must
/// never live inside `app.db`: a future backup stage walks the data
/// directory and must exclude it by name. The file name convention is
/// `device-auth.json` directly under the caller's data directory; restore
/// must not replace it, reset wipes it only on full-data reset, and a Host
/// without this file authenticates nothing until fresh pairing mints new
/// material.
pub struct FileDeviceAuthStore {
    /// Location of the protected JSON file; the parent directory is owned by
    /// the caller.
    path: PathBuf,
}

impl core::fmt::Debug for FileDeviceAuthStore {
    /// Renders the path and the entry count only.
    ///
    /// The read is best-effort: when the file cannot be read or parsed, the
    /// count renders as `"unreadable"` instead of failing. Secrets and
    /// descriptors never appear here.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.read_entries() {
            Ok(entries) => f
                .debug_struct("FileDeviceAuthStore")
                .field("path", &self.path)
                .field("entries", &entries.len())
                .finish(),
            Err(_) => f
                .debug_struct("FileDeviceAuthStore")
                .field("path", &self.path)
                .field("entries", &"unreadable")
                .finish(),
        }
    }
}

impl FileDeviceAuthStore {
    /// Opens the protected device-auth file at `path`.
    ///
    /// The caller owns directory creation: opening fails when the parent
    /// directory is missing. A missing file opens as an empty store and is
    /// created lazily on the first save. An existing file keeps its bytes
    /// untouched, but on Unix its mode is verified (and tightened to `0600`
    /// when lax; see the type-level contract). Paths naming no file, and
    /// paths naming a directory, are rejected.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialTechnicalError::StorageUnavailable`] when the path
    /// names no file, the parent directory is missing, the path is a
    /// directory, file metadata cannot be read, or Unix permissions cannot
    /// be tightened to owner-only. Error reasons carry the path only, never
    /// file content.
    pub fn open(path: &Path) -> Result<Self, CredentialTechnicalError> {
        let shown = path.display();
        if path.file_name().is_none() {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("device-auth path {shown} names no file"),
            });
        }
        let parent = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            Some(_) | None => Path::new("."),
        };
        if !parent.is_dir() {
            let parent_shown = parent.display();
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("device-auth directory {parent_shown} is missing"),
            });
        }
        if path.exists() {
            if path.is_dir() {
                return Err(CredentialTechnicalError::StorageUnavailable {
                    reason: format!("device-auth path {shown} is a directory"),
                });
            }
            enforce_owner_only(path)?;
        }
        Ok(Self {
            path: path.to_owned(),
        })
    }

    /// Persists `secret` for `device`, creating or rotating its entry.
    ///
    /// The caller mints the secret; this method performs no strength
    /// validation on it, it only takes custody. The entry's `descriptor` is
    /// the owner-visible display string and `paired_at` is stamped with the
    /// write time for display and audit only (it is not the pairing record's
    /// pairing time). The write goes through the atomic temp-plus-rename
    /// path; concurrent approves must be owner-serialized by the caller.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialTechnicalError::StorageUnavailable`] when the
    /// file cannot be read (including a malformed existing file), the
    /// staging temp cannot be written, or the atomic replace fails.
    pub fn save_secret(
        &self,
        device: &DeviceId,
        descriptor: &str,
        secret: &str,
    ) -> Result<(), CredentialTechnicalError> {
        let mut entries = self.read_entries()?;
        entries.insert(
            device_key(device),
            StoredDeviceAuth {
                secret_hex: encode_hex_lower(secret.as_bytes()),
                descriptor: descriptor.to_owned(),
                paired_at: WallClockWithTz::now().to_rfc3339(),
            },
        );
        self.write_entries(&entries)
    }

    /// Loads the persisted secret for `device`, if any.
    ///
    /// Every call reads the file through: there is no cache, so a rotation
    /// or revocation persisted by another process is observed immediately.
    /// An unknown device (or a missing file) yields `Ok(None)`; only an
    /// unreadable or malformed file yields an error, never a silent default.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialTechnicalError::StorageUnavailable`] when the
    /// file cannot be read or fails validation.
    pub fn load_secret(
        &self,
        device: &DeviceId,
    ) -> Result<Option<SecretValue>, CredentialTechnicalError> {
        let entries = self.read_entries()?;
        let key = device_key(device);
        let Some(entry) = entries.get(&key) else {
            return Ok(None);
        };
        let Some(bytes) = decode_hex_lower(&entry.secret_hex) else {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("device-auth entry for {key} holds malformed secret material"),
            });
        };
        Ok(Some(SecretValue::new(bytes)))
    }

    /// Revokes `device` by deleting its entry from the protected file.
    ///
    /// This is the durable half of device revocation: once the atomic
    /// rewrite completes, no process loading through this store will verify
    /// proofs for the device again. Deleting an unknown device — or deleting
    /// while the file is missing — succeeds without writing anything.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialTechnicalError::StorageUnavailable`] when the
    /// file cannot be read (including a malformed existing file) or the
    /// atomic rewrite fails.
    pub fn delete_for(&self, device: &DeviceId) -> Result<(), CredentialTechnicalError> {
        let mut entries = self.read_entries()?;
        if entries.remove(&device_key(device)).is_none() {
            return Ok(());
        }
        self.write_entries(&entries)
    }

    // Reads and validates the whole file; a missing file reads as empty.
    fn read_entries(&self) -> Result<BTreeMap<String, StoredDeviceAuth>, CredentialTechnicalError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new());
            }
            Err(err) => {
                let shown = self.path.display();
                return Err(CredentialTechnicalError::StorageUnavailable {
                    reason: format!("device-auth file {shown} is unreadable: {err}"),
                });
            }
        };
        let parsed = parse_device_auth_file(&bytes).map_err(|detail| {
            let shown = self.path.display();
            CredentialTechnicalError::StorageUnavailable {
                reason: format!("device-auth file {shown} is malformed: {detail}"),
            }
        })?;
        let mut entries = BTreeMap::new();
        for (device_text, entry) in parsed {
            let Some(device) = parse_device_key(&device_text) else {
                let shown = self.path.display();
                return Err(CredentialTechnicalError::StorageUnavailable {
                    reason: format!("device-auth file {shown} holds an invalid device key"),
                });
            };
            if decode_hex_lower(&entry.secret_hex).is_none() {
                let shown = self.path.display();
                return Err(CredentialTechnicalError::StorageUnavailable {
                    reason: format!("device-auth file {shown} holds malformed secret material"),
                });
            }
            if WallClockWithTz::parse_rfc3339(&entry.paired_at).is_err() {
                let shown = self.path.display();
                return Err(CredentialTechnicalError::StorageUnavailable {
                    reason: format!("device-auth file {shown} holds an invalid timestamp"),
                });
            }
            if entries.insert(device_key(&device), entry).is_some() {
                let shown = self.path.display();
                return Err(CredentialTechnicalError::StorageUnavailable {
                    reason: format!("device-auth file {shown} holds a duplicate device entry"),
                });
            }
        }
        Ok(entries)
    }

    // Renders the entries and replaces the file via temp-plus-rename in the
    // same directory, so readers never observe a partial document.
    fn write_entries(
        &self,
        entries: &BTreeMap<String, StoredDeviceAuth>,
    ) -> Result<(), CredentialTechnicalError> {
        let shown = self.path.display();
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let tmp = self.path.with_extension(format!("tmp.{pid}.{nanos}"));
        let rendered = render_device_auth_file(entries);
        if let Err(err) = stage_file(&tmp, rendered.as_bytes()) {
            remove_best_effort(&tmp);
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("device-auth write to {shown} failed: {err}"),
            });
        }
        if let Err(err) = std::fs::rename(&tmp, &self.path) {
            remove_best_effort(&tmp);
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("device-auth write to {shown} failed: {err}"),
            });
        }
        enforce_owner_only(&self.path)
    }
}

// One validated file entry: secret bytes as lowercase hex plus display and
// timing metadata. Keys live in the surrounding map, canonicalized to
// hyphenated UUID text.
struct StoredDeviceAuth {
    secret_hex: String,
    descriptor: String,
    paired_at: String,
}

// Renders `device` as canonical hyphenated UUID text for use as a file key.
fn device_key(device: &DeviceId) -> String {
    device.0.as_uuid().to_string()
}

// Parses a file key back into a `DeviceId`; yields `None` for anything that
// is not UUID text. The backing UUID type is inferred through
// `RawId::from_uuid` and never named, so this stays on the existing
// dependency set.
fn parse_device_key(text: &str) -> Option<DeviceId> {
    text.parse().ok().map(RawId::from_uuid).map(DeviceId)
}

// Enforces owner-only mode on an existing file (Unix): already-`0600` passes
// through, anything else is tightened, and a tighten that does not stick is
// an error. Non-Unix has no mode to enforce; the call still succeeds so the
// documented directory-level protection applies instead.
#[cfg(unix)]
fn enforce_owner_only(path: &Path) -> Result<(), CredentialTechnicalError> {
    let shown = path.display();
    let current =
        std::fs::metadata(path).map_err(|err| CredentialTechnicalError::StorageUnavailable {
            reason: format!("device-auth file {shown} is unreadable: {err}"),
        })?;
    if current.permissions().mode() & 0o777 == 0o600 {
        return Ok(());
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|err| {
        CredentialTechnicalError::StorageUnavailable {
            reason: format!("device-auth file {shown} cannot be tightened to owner-only: {err}"),
        }
    })?;
    let tightened =
        std::fs::metadata(path).map_err(|err| CredentialTechnicalError::StorageUnavailable {
            reason: format!("device-auth file {shown} is unreadable: {err}"),
        })?;
    if tightened.permissions().mode() & 0o777 != 0o600 {
        return Err(CredentialTechnicalError::StorageUnavailable {
            reason: format!("device-auth file {shown} cannot be tightened to owner-only"),
        });
    }
    Ok(())
}

// Non-Unix platforms have no Unix mode bits; protection rests on the
// caller-owned directory, as documented on the store.
#[cfg(not(unix))]
fn enforce_owner_only(_path: &Path) -> Result<(), CredentialTechnicalError> {
    Ok(())
}

// Stages the new document to a temp file in the same directory. On Unix the
// temp is created `0600` so secrets are never briefly world-readable;
// `sync_all` keeps a crash from leaving a truncated temp behind.
fn stage_file(tmp: &Path, rendered: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut staged = options.open(tmp)?;
    std::io::Write::write_all(&mut staged, rendered)?;
    staged.sync_all()
}

// Best-effort temp cleanup after a failed write; leftovers stay in the same
// directory for the owner to notice and remove.
fn remove_best_effort(tmp: &Path) {
    if std::fs::remove_file(tmp).is_err() {
        // Nothing to do: the temp carries no trust beyond the store file
        // itself, and reporting cleanup failure would mask the real error.
    }
}

// Renders the whole document in one canonical shape: entries ordered by
// device key, no whitespace, trailing newline.
fn render_device_auth_file(entries: &BTreeMap<String, StoredDeviceAuth>) -> String {
    let mut out = String::from("{\"devices\":{");
    let mut first = true;
    for (device, entry) in entries {
        if !first {
            out.push(',');
        }
        first = false;
        append_json_string(&mut out, device);
        out.push_str(":{\"secret_hex\":");
        append_json_string(&mut out, &entry.secret_hex);
        out.push_str(",\"descriptor\":");
        append_json_string(&mut out, &entry.descriptor);
        out.push_str(",\"paired_at\":");
        append_json_string(&mut out, &entry.paired_at);
        out.push('}');
    }
    out.push_str("}}\n");
    out
}

// Appends one JSON string literal with escaping for quotes, backslashes, and
// control characters; other characters (including non-ASCII) pass through.
fn append_json_string(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ch if (ch as u32) < 0x20 => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let scalar = ch as u32;
                out.push_str("\\u");
                out.push(HEX[((scalar >> 12) & 0x0f) as usize] as char);
                out.push(HEX[((scalar >> 8) & 0x0f) as usize] as char);
                out.push(HEX[((scalar >> 4) & 0x0f) as usize] as char);
                out.push(HEX[(scalar & 0x0f) as usize] as char);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

// Parses the whole document into raw `(key, entry)` pairs; semantic checks
// (UUID keys, hex secrets, RFC 3339 timestamps) happen in `read_entries`.
// Every error is positional or structural, never file content.
fn parse_device_auth_file(bytes: &[u8]) -> Result<Vec<(String, StoredDeviceAuth)>, String> {
    let text = core::str::from_utf8(bytes).map_err(|_| "file is not valid UTF-8".to_owned())?;
    JsonCursor {
        text: text.as_bytes(),
        pos: 0,
    }
    .parse_file()
}

// Byte cursor over the document for the fixed-shape parser below.
struct JsonCursor<'a> {
    text: &'a [u8],
    pos: usize,
}

impl JsonCursor<'_> {
    // Parses `{"devices": {<entry>, ...}}` with only whitespace elsewhere.
    fn parse_file(&mut self) -> Result<Vec<(String, StoredDeviceAuth)>, String> {
        self.skip_ws();
        self.expect_byte(b'{')?;
        self.skip_ws();
        let section = self.parse_string()?;
        if section != "devices" {
            return Err(self.err("expected the devices section"));
        }
        self.skip_ws();
        self.expect_byte(b':')?;
        self.skip_ws();
        self.expect_byte(b'{')?;
        let mut entries = Vec::new();
        self.skip_ws();
        if self.consume_byte(b'}') {
            self.skip_ws();
            self.expect_byte(b'}')?;
            self.skip_ws();
            self.expect_end()?;
            return Ok(entries);
        }
        loop {
            self.skip_ws();
            let device = self.parse_string()?;
            self.skip_ws();
            self.expect_byte(b':')?;
            self.skip_ws();
            let entry = self.parse_entry()?;
            if entries.iter().any(|(known, _)| *known == device) {
                return Err(self.err("duplicate device entry"));
            }
            entries.push((device, entry));
            self.skip_ws();
            if self.consume_byte(b',') {
                continue;
            }
            self.expect_byte(b'}')?;
            break;
        }
        self.skip_ws();
        self.expect_byte(b'}')?;
        self.skip_ws();
        self.expect_end()?;
        Ok(entries)
    }

    // Parses one entry object holding exactly the required string fields.
    fn parse_entry(&mut self) -> Result<StoredDeviceAuth, String> {
        self.expect_byte(b'{')?;
        let mut secret_hex: Option<String> = None;
        let mut descriptor: Option<String> = None;
        let mut paired_at: Option<String> = None;
        self.skip_ws();
        if self.consume_byte(b'}') {
            return Err(self.err("device entry is missing required fields"));
        }
        loop {
            self.skip_ws();
            let field = self.parse_string()?;
            self.skip_ws();
            self.expect_byte(b':')?;
            self.skip_ws();
            let value = self.parse_string()?;
            if field == "secret_hex" {
                if secret_hex.is_some() {
                    return Err(self.err("duplicate device field"));
                }
                secret_hex = Some(value);
            } else if field == "descriptor" {
                if descriptor.is_some() {
                    return Err(self.err("duplicate device field"));
                }
                descriptor = Some(value);
            } else if field == "paired_at" {
                if paired_at.is_some() {
                    return Err(self.err("duplicate device field"));
                }
                paired_at = Some(value);
            } else {
                return Err(self.err("unknown device field"));
            }
            self.skip_ws();
            if self.consume_byte(b',') {
                continue;
            }
            self.expect_byte(b'}')?;
            break;
        }
        let (Some(secret_hex), Some(descriptor), Some(paired_at)) =
            (secret_hex, descriptor, paired_at)
        else {
            return Err(self.err("device entry is missing required fields"));
        };
        Ok(StoredDeviceAuth {
            secret_hex,
            descriptor,
            paired_at,
        })
    }

    // Parses one JSON string literal with escapes (including surrogate
    // pairs); raw control characters and lone surrogates are rejected.
    fn parse_string(&mut self) -> Result<String, String> {
        self.expect_byte(b'"')?;
        let mut out = String::new();
        loop {
            let byte = self
                .text
                .get(self.pos)
                .copied()
                .ok_or_else(|| self.err("unexpected end inside a string"))?;
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    self.parse_escape_into(&mut out)?;
                }
                byte if byte < 0x20 => {
                    return Err(self.err("unescaped control character in string"));
                }
                _ => {
                    let rest = core::str::from_utf8(&self.text[self.pos..])
                        .map_err(|_| self.err("string is not valid UTF-8"))?;
                    let Some(ch) = rest.chars().next() else {
                        return Err(self.err("unexpected end inside a string"));
                    };
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }

    // Parses one escape (the byte after the backslash was already consumed
    // by advancing past it) into `out`.
    fn parse_escape_into(&mut self, out: &mut String) -> Result<(), String> {
        let esc = self
            .text
            .get(self.pos)
            .copied()
            .ok_or_else(|| self.err("unexpected end inside a string escape"))?;
        self.pos += 1;
        match esc {
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'/' => out.push('/'),
            b'b' => out.push('\u{8}'),
            b'f' => out.push('\u{c}'),
            b'n' => out.push('\n'),
            b'r' => out.push('\r'),
            b't' => out.push('\t'),
            b'u' => {
                let high = self.parse_hex4()?;
                if (0xd800..0xdc00).contains(&high) {
                    if self.text.get(self.pos).copied() != Some(b'\\') {
                        return Err(self.err("lone surrogate escape"));
                    }
                    self.pos += 1;
                    if self.text.get(self.pos).copied() != Some(b'u') {
                        return Err(self.err("lone surrogate escape"));
                    }
                    self.pos += 1;
                    let low = self.parse_hex4()?;
                    if !(0xdc00..0xe000).contains(&low) {
                        return Err(self.err("lone surrogate escape"));
                    }
                    let scalar = 0x1_0000 + ((high - 0xd800) << 10) + (low - 0xdc00);
                    let Some(ch) = char::from_u32(scalar) else {
                        return Err(self.err("invalid string escape"));
                    };
                    out.push(ch);
                } else if (0xdc00..0xe000).contains(&high) {
                    return Err(self.err("lone surrogate escape"));
                } else {
                    let Some(ch) = char::from_u32(high) else {
                        return Err(self.err("invalid string escape"));
                    };
                    out.push(ch);
                }
            }
            _ => return Err(self.err("invalid string escape")),
        }
        Ok(())
    }

    // Parses exactly four hex digits into a scalar value.
    fn parse_hex4(&mut self) -> Result<u32, String> {
        let mut value: u32 = 0;
        for _ in 0..4 {
            let byte = self
                .text
                .get(self.pos)
                .copied()
                .ok_or_else(|| self.err("unexpected end inside a string escape"))?;
            let Some(digit) = hex_val(byte) else {
                return Err(self.err("invalid string escape"));
            };
            value = (value << 4) | u32::from(digit);
            self.pos += 1;
        }
        Ok(value)
    }

    fn skip_ws(&mut self) {
        while matches!(
            self.text.get(self.pos).copied(),
            Some(b' ' | b'\t' | b'\n' | b'\r')
        ) {
            self.pos += 1;
        }
    }

    fn expect_byte(&mut self, want: u8) -> Result<(), String> {
        if self.text.get(self.pos).copied() == Some(want) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err("unexpected input"))
        }
    }

    fn consume_byte(&mut self, want: u8) -> bool {
        if self.text.get(self.pos).copied() == Some(want) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_end(&self) -> Result<(), String> {
        if self.pos == self.text.len() {
            Ok(())
        } else {
            Err(self.err("trailing input after the device-auth document"))
        }
    }

    // Renders a positional, content-free error for the current cursor.
    fn err(&self, detail: &str) -> String {
        let offset = self.pos;
        format!("{detail} at byte {offset}")
    }
}

#[cfg(test)]
mod tests {
    use super::FileDeviceAuthStore;
    use super::{
        CredentialRef, CredentialStore, CredentialTechnicalError, MemoryCredentialStore,
        RegisterCredentialCommand, RegisterOutcome, credential_availability, register,
    };
    use super::{
        DeviceId, DevicePairingRepository, DevicePairingStatus, DeviceRecord, PendingPairing,
    };
    use ene_primitive::{RawId, WallClockWithTz};
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    struct FakeRepo {
        refs: Mutex<HashMap<(String, String), CredentialRef>>,
        saves: Mutex<u64>,
    }

    impl FakeRepo {
        fn new() -> Self {
            Self {
                refs: Mutex::new(HashMap::new()),
                saves: Mutex::new(0),
            }
        }

        async fn save_count(&self) -> u64 {
            *self.saves.lock().await
        }
    }

    impl super::CredentialRefRepository for FakeRepo {
        async fn save_ref(&self, cred: CredentialRef) -> Result<(), CredentialTechnicalError> {
            let mut refs = self.refs.lock().await;
            refs.insert((cred.provider.clone(), cred.label.clone()), cred);
            let mut saves = self.saves.lock().await;
            *saves += 1;
            Ok(())
        }

        async fn load_ref(
            &self,
            provider: &str,
            label: &str,
        ) -> Result<Option<CredentialRef>, CredentialTechnicalError> {
            let refs = self.refs.lock().await;
            Ok(refs.get(&(provider.to_owned(), label.to_owned())).cloned())
        }

        async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError> {
            let refs = self.refs.lock().await;
            Ok(refs.values().cloned().collect())
        }
    }

    fn command() -> RegisterCredentialCommand {
        RegisterCredentialCommand {
            provider: "acme".to_owned(),
            label: "main".to_owned(),
        }
    }

    #[tokio::test]
    async fn register_persists_a_new_ref() {
        let repo = FakeRepo::new();
        let outcome = register(command(), &repo).await;
        assert!(matches!(outcome, Ok(RegisterOutcome::Registered(_))));
        assert_eq!(repo.save_count().await, 1);
    }

    #[tokio::test]
    async fn register_rejects_a_blank_provider() {
        let repo = FakeRepo::new();
        let cmd = RegisterCredentialCommand {
            provider: "   ".to_owned(),
            label: "main".to_owned(),
        };
        let outcome = register(cmd, &repo).await;
        assert!(matches!(outcome, Ok(RegisterOutcome::InvalidProvider)));
        assert_eq!(repo.save_count().await, 0);
    }

    #[tokio::test]
    async fn re_register_returns_the_existing_ref_without_overwriting() {
        let repo = FakeRepo::new();
        let first_outcome = register(command(), &repo).await;
        assert!(matches!(first_outcome, Ok(RegisterOutcome::Registered(_))));
        let Ok(RegisterOutcome::Registered(first)) = first_outcome else {
            return;
        };
        let second_outcome = register(command(), &repo).await;
        assert!(matches!(
            second_outcome,
            Ok(RegisterOutcome::AlreadyExists(_))
        ));
        let Ok(RegisterOutcome::AlreadyExists(existing)) = second_outcome else {
            return;
        };
        assert_eq!(existing, first);
        assert_eq!(repo.save_count().await, 1);
    }

    #[tokio::test]
    async fn ref_equality_ignores_nothing() {
        let repo = FakeRepo::new();
        let outcome = register(command(), &repo).await;
        assert!(matches!(outcome, Ok(RegisterOutcome::Registered(_))));
        let Ok(RegisterOutcome::Registered(cred)) = outcome else {
            return;
        };
        assert_eq!(
            cred,
            CredentialRef {
                id: "acme:main".to_owned(),
                provider: "acme".to_owned(),
                label: "main".to_owned(),
            }
        );
    }

    #[test]
    fn availability_requires_both_registry_and_store() {
        let store = MemoryCredentialStore::new();
        let cred = CredentialRef {
            id: "acme:main".to_owned(),
            provider: "acme".to_owned(),
            label: "main".to_owned(),
        };
        let missing = credential_availability(&cred, false, &store);
        assert!(!missing.present);
        assert_eq!(missing.credential, None);
        store.insert(cred.clone(), "bearer-token");
        let store_only = credential_availability(&cred, false, &store);
        assert!(!store_only.present);
        let both = credential_availability(&cred, true, &store);
        assert!(both.present);
        assert_eq!(both.credential, Some(cred.clone()));
        let removed = store.delete(&cred);
        assert!(removed.is_ok());
        let after_delete = credential_availability(&cred, true, &store);
        assert!(!after_delete.present);
        assert_eq!(after_delete.credential, Some(cred));
    }

    #[test]
    fn bearer_closure_receives_the_inserted_secret() {
        let store = MemoryCredentialStore::new();
        let cred = CredentialRef {
            id: "acme:main".to_owned(),
            provider: "acme".to_owned(),
            label: "main".to_owned(),
        };
        store.insert(cred.clone(), "bearer-token");
        let seen = store.with_bearer(&cred, str::len);
        assert_eq!(seen, Ok("bearer-token".len()));
    }

    #[test]
    fn public_debug_output_carries_no_secret() {
        let cred = CredentialRef {
            id: "acme:main".to_owned(),
            provider: "acme".to_owned(),
            label: "main".to_owned(),
        };
        let rendered = format!("{cred:?}");
        assert!(rendered.contains("acme"));
        assert!(!rendered.contains("bearer-token"));
    }

    struct FakePairingRepo {
        pending: Mutex<HashMap<String, PendingPairing>>,
        paired: Mutex<HashMap<String, DeviceRecord>>,
    }

    impl FakePairingRepo {
        fn new() -> Self {
            Self {
                pending: Mutex::new(HashMap::new()),
                paired: Mutex::new(HashMap::new()),
            }
        }
    }

    impl DevicePairingRepository for FakePairingRepo {
        async fn request_pairing(
            &self,
            descriptor: String,
        ) -> Result<DevicePairingStatus, CredentialTechnicalError> {
            if let Some(device) = self.paired.lock().await.get(&descriptor).cloned() {
                return Ok(DevicePairingStatus::Paired { device });
            }
            if let Some(pending) = self.pending.lock().await.get(&descriptor).cloned() {
                return Ok(DevicePairingStatus::Pending { pending });
            }
            let pending = PendingPairing {
                descriptor: descriptor.clone(),
                requested_at: WallClockWithTz::now(),
            };
            self.pending
                .lock()
                .await
                .insert(descriptor, pending.clone());
            Ok(DevicePairingStatus::Pending { pending })
        }

        async fn approve_pending(
            &self,
            descriptor: &str,
        ) -> Result<Option<(DeviceRecord, String)>, CredentialTechnicalError> {
            if let Some(device) = self.paired.lock().await.get(descriptor).cloned() {
                let secret = RawId::new().as_uuid().to_string();
                return Ok(Some((device, secret)));
            }
            let pending = self.pending.lock().await.remove(descriptor);
            let Some(stored) = pending else {
                return Ok(None);
            };
            let device = DeviceRecord {
                id: DeviceId(RawId::new()),
                descriptor: stored.descriptor,
                paired_at: WallClockWithTz::now(),
            };
            self.paired
                .lock()
                .await
                .insert(descriptor.to_owned(), device.clone());
            let secret = RawId::new().as_uuid().to_string();
            Ok(Some((device, secret)))
        }

        async fn find_device(
            &self,
            id: &DeviceId,
        ) -> Result<Option<DeviceRecord>, CredentialTechnicalError> {
            let paired = self.paired.lock().await;
            Ok(paired.values().find(|device| device.id == *id).cloned())
        }

        async fn list_pending(&self) -> Result<Vec<PendingPairing>, CredentialTechnicalError> {
            let pending = self.pending.lock().await;
            Ok(pending.values().cloned().collect())
        }
    }

    fn assert_uuid_text_shape(secret: &str) {
        assert_eq!(secret.len(), 36);
        for index in [8_usize, 13, 18, 23] {
            assert!(matches!(secret.as_bytes().get(index), Some(b'-')));
        }
    }

    #[tokio::test]
    async fn first_request_yields_pending() {
        let repo = FakePairingRepo::new();
        let status = repo.request_pairing("phone".to_owned()).await;
        assert!(matches!(status, Ok(DevicePairingStatus::Pending { .. })));
        let Ok(DevicePairingStatus::Pending { pending }) = status else {
            return;
        };
        assert_eq!(pending.descriptor.as_str(), "phone");
        let listed = repo.list_pending().await;
        let Ok(items) = listed else {
            return;
        };
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn second_request_for_same_descriptor_stays_pending_without_duplicate() {
        let repo = FakePairingRepo::new();
        let first = repo.request_pairing("phone".to_owned()).await;
        assert!(matches!(first, Ok(DevicePairingStatus::Pending { .. })));
        let second = repo.request_pairing("phone".to_owned()).await;
        assert!(matches!(second, Ok(DevicePairingStatus::Pending { .. })));
        let Ok(DevicePairingStatus::Pending {
            pending: first_pending,
        }) = first
        else {
            return;
        };
        let Ok(DevicePairingStatus::Pending {
            pending: second_pending,
        }) = second
        else {
            return;
        };
        assert_eq!(first_pending.descriptor, second_pending.descriptor);
        let listed = repo.list_pending().await;
        let Ok(items) = listed else {
            return;
        };
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn approve_unknown_descriptor_returns_none() {
        let repo = FakePairingRepo::new();
        let approved = repo.approve_pending("unknown").await;
        assert!(matches!(approved, Ok(None)));
        let found = repo.find_device(&DeviceId(RawId::new())).await;
        assert!(matches!(found, Ok(None)));
    }

    #[tokio::test]
    async fn approve_moves_pending_to_paired() {
        let repo = FakePairingRepo::new();
        let requested = repo.request_pairing("phone".to_owned()).await;
        assert!(matches!(requested, Ok(DevicePairingStatus::Pending { .. })));
        let approved = repo.approve_pending("phone").await;
        assert!(matches!(approved, Ok(Some(_))));
        let Ok(Some((device, secret))) = approved else {
            return;
        };
        assert_eq!(device.descriptor.as_str(), "phone");
        assert_uuid_text_shape(&secret);
        let listed = repo.list_pending().await;
        let Ok(items) = listed else {
            return;
        };
        assert!(items.is_empty());
        let found = repo.find_device(&device.id).await;
        assert!(matches!(found, Ok(Some(_))));
        let Ok(Some(stored)) = found else {
            return;
        };
        assert_eq!(stored, device);
    }

    #[tokio::test]
    async fn re_request_after_paired_returns_paired_with_same_id() {
        let repo = FakePairingRepo::new();
        let requested = repo.request_pairing("phone".to_owned()).await;
        assert!(matches!(requested, Ok(DevicePairingStatus::Pending { .. })));
        let approved = repo.approve_pending("phone").await;
        let Ok(Some((device, _))) = approved else {
            return;
        };
        let again = repo.request_pairing("phone".to_owned()).await;
        assert!(matches!(again, Ok(DevicePairingStatus::Paired { .. })));
        let Ok(DevicePairingStatus::Paired { device: existing }) = again else {
            return;
        };
        assert_eq!(existing.id, device.id);
        assert_eq!(existing, device);
    }

    #[tokio::test]
    async fn re_approve_returns_the_existing_record_with_a_fresh_secret() {
        let repo = FakePairingRepo::new();
        let requested = repo.request_pairing("phone".to_owned()).await;
        assert!(matches!(requested, Ok(DevicePairingStatus::Pending { .. })));
        let approved = repo.approve_pending("phone").await;
        let Ok(Some((first, first_secret))) = approved else {
            return;
        };
        let reapproved = repo.approve_pending("phone").await;
        assert!(matches!(reapproved, Ok(Some(_))));
        let Ok(Some((second, second_secret))) = reapproved else {
            return;
        };
        assert_eq!(second.id, first.id);
        assert_eq!(second, first);
        assert_uuid_text_shape(&second_secret);
        assert_ne!(first_secret, second_secret);
    }

    #[tokio::test]
    async fn list_pending_reports_each_descriptor_once() {
        let repo = FakePairingRepo::new();
        let first = repo.request_pairing("phone".to_owned()).await;
        assert!(matches!(first, Ok(DevicePairingStatus::Pending { .. })));
        let second = repo.request_pairing("tablet".to_owned()).await;
        assert!(matches!(second, Ok(DevicePairingStatus::Pending { .. })));
        let listed = repo.list_pending().await;
        let Ok(items) = listed else {
            return;
        };
        let descriptors: Vec<&str> = items.iter().map(|item| item.descriptor.as_str()).collect();
        assert_eq!(descriptors.len(), 2);
        assert!(descriptors.contains(&"phone"));
        assert!(descriptors.contains(&"tablet"));
    }

    #[test]
    fn pairing_proof_matches_rfc4231_case_1() {
        let key = "\x0b".repeat(20);
        let proof = super::pairing_proof_hex(&key, "Hi There");
        assert_eq!(
            proof.as_str(),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert!(super::verify_pairing_proof(&key, "Hi There", &proof));
    }

    #[test]
    fn pairing_proof_round_trips_and_rejects_mismatch() {
        let proof = super::pairing_proof_hex("pairing-secret", "single-use-nonce");
        assert!(super::verify_pairing_proof(
            "pairing-secret",
            "single-use-nonce",
            &proof
        ));
        assert!(!super::verify_pairing_proof(
            "other-secret",
            "single-use-nonce",
            &proof
        ));
        assert!(!super::verify_pairing_proof(
            "pairing-secret",
            "other-nonce",
            &proof
        ));
        assert!(!super::verify_pairing_proof(
            "pairing-secret",
            "single-use-nonce",
            "not-hex!!"
        ));
        assert!(!super::verify_pairing_proof(
            "pairing-secret",
            "single-use-nonce",
            ""
        ));
        assert!(!super::verify_pairing_proof(
            "pairing-secret",
            "single-use-nonce",
            "B0344C61D8DB38535CA8AFCEAF0BF12B881DC200C9833DA726E9376C2E32CFF7"
        ));
    }

    #[test]
    fn pairing_proof_rejects_tampered_hex() {
        let proof = super::pairing_proof_hex("pairing-secret", "single-use-nonce");
        let tampered: String = proof
            .chars()
            .enumerate()
            .map(|(index, digit)| {
                if index == 0 {
                    if digit == '0' { '1' } else { '0' }
                } else {
                    digit
                }
            })
            .collect();
        assert_ne!(tampered, proof);
        assert!(!super::verify_pairing_proof(
            "pairing-secret",
            "single-use-nonce",
            &tampered
        ));
    }

    fn fresh_tempdir() -> Option<tempfile::TempDir> {
        let temp = tempfile::tempdir();
        assert!(temp.is_ok(), "tempdir must be available");
        temp.ok()
    }

    fn open_device_auth_store(path: &std::path::Path) -> Option<FileDeviceAuthStore> {
        let opened = FileDeviceAuthStore::open(path);
        assert!(opened.is_ok(), "device-auth store must open");
        opened.ok()
    }

    #[test]
    fn device_auth_roundtrip_preserves_secret_bytes() {
        let Some(temp) = fresh_tempdir() else {
            return;
        };
        let path = temp.path().join("device-auth.json");
        let Some(store) = open_device_auth_store(&path) else {
            return;
        };
        let device = DeviceId(RawId::new());
        let saved = store.save_secret(&device, "phone", "pairing-secret-value");
        assert!(saved.is_ok(), "save must succeed");
        let loaded = store.load_secret(&device);
        assert!(loaded.is_ok(), "load must succeed");
        let Ok(Some(secret)) = loaded else {
            return;
        };
        assert_eq!(secret.bytes(), "pairing-secret-value".as_bytes());
    }

    #[test]
    fn device_auth_missing_file_loads_none_and_missing_parent_fails_open() {
        let Some(temp) = fresh_tempdir() else {
            return;
        };
        let path = temp.path().join("device-auth.json");
        let Some(store) = open_device_auth_store(&path) else {
            return;
        };
        let loaded = store.load_secret(&DeviceId(RawId::new()));
        assert!(matches!(loaded, Ok(None)));
        let nested = temp.path().join("no-such-dir").join("device-auth.json");
        assert!(FileDeviceAuthStore::open(&nested).is_err());
    }

    #[test]
    fn device_auth_malformed_files_error_never_default() {
        let entry = "{\"secret_hex\":\"00\",\"descriptor\":\"d\",\
             \"paired_at\":\"2026-09-08T12:00:00+09:00\"}";
        let key_prefix = "{\"devices\":{\"123e4567-e89b-12d3-a456-426614174000\":";
        let keyed = key_prefix.to_owned() + entry + "}}";
        let bad_key = "{\"devices\":{\"not-a-uuid\":".to_owned() + entry + "}}";
        let extra_field = key_prefix.to_owned() + entry + ",\"extra\":\"x\"}}";
        let duplicate_key = key_prefix.to_owned()
            + entry
            + ",\"123e4567-e89b-12d3-a456-426614174000\":"
            + entry
            + "}}";
        let fixtures = [
            "not json{{{".to_owned(),
            "[]".to_owned(),
            "{}".to_owned(),
            "{\"other\":{}}".to_owned(),
            "{\"devices\":[]}".to_owned(),
            "{\"devices\":{}}trailing".to_owned(),
            bad_key,
            "{\"devices\":{\"123e4567-e89b-12d3-a456-426614174000\":\
             {\"secret_hex\":\"zz\",\"descriptor\":\"d\",\
             \"paired_at\":\"2026-09-08T12:00:00+09:00\"}}}"
                .to_owned(),
            "{\"devices\":{\"123e4567-e89b-12d3-a456-426614174000\":\
             {\"secret_hex\":\"AABB\",\"descriptor\":\"d\",\
             \"paired_at\":\"2026-09-08T12:00:00+09:00\"}}}"
                .to_owned(),
            "{\"devices\":{\"123e4567-e89b-12d3-a456-426614174000\":\
             {\"secret_hex\":\"00\",\"descriptor\":\"d\",\"paired_at\":\"yesterday\"}}}"
                .to_owned(),
            "{\"devices\":{\"123e4567-e89b-12d3-a456-426614174000\":\
             {\"secret_hex\":\"00\",\"descriptor\":\"d\"}}}"
                .to_owned(),
            extra_field,
            keyed.clone() + "}]",
            duplicate_key,
        ];
        let Some(temp) = fresh_tempdir() else {
            return;
        };
        let path = temp.path().join("device-auth.json");
        for fixture in fixtures {
            let written = std::fs::write(&path, &fixture);
            assert!(written.is_ok(), "fixture setup must succeed");
            let Some(store) = open_device_auth_store(&path) else {
                return;
            };
            let device = DeviceId(RawId::new());
            assert!(
                store.load_secret(&device).is_err(),
                "malformed file must error, never default"
            );
            assert!(
                store.save_secret(&device, "phone", "secret").is_err(),
                "saving over a malformed file must error, never clobber"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn device_auth_open_tightens_lax_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let Some(temp) = fresh_tempdir() else {
            return;
        };
        let path = temp.path().join("device-auth.json");
        let written = std::fs::write(&path, "{\"devices\":{}}");
        assert!(written.is_ok(), "fixture setup must succeed");
        let lax = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
        assert!(lax.is_ok(), "fixture setup must succeed");
        let Some(store) = open_device_auth_store(&path) else {
            return;
        };
        let meta = std::fs::metadata(&path);
        assert!(meta.is_ok(), "metadata must be readable");
        let Ok(meta) = meta else {
            return;
        };
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        let loaded = store.load_secret(&DeviceId(RawId::new()));
        assert!(matches!(loaded, Ok(None)));
    }

    #[cfg(unix)]
    #[test]
    fn device_auth_saved_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let Some(temp) = fresh_tempdir() else {
            return;
        };
        let path = temp.path().join("device-auth.json");
        let Some(store) = open_device_auth_store(&path) else {
            return;
        };
        let saved = store.save_secret(&DeviceId(RawId::new()), "phone", "pairing-secret");
        assert!(saved.is_ok(), "save must succeed");
        let meta = std::fs::metadata(&path);
        assert!(meta.is_ok(), "metadata must be readable");
        let Ok(meta) = meta else {
            return;
        };
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn device_auth_delete_removes_only_the_target() {
        let Some(temp) = fresh_tempdir() else {
            return;
        };
        let path = temp.path().join("device-auth.json");
        let Some(store) = open_device_auth_store(&path) else {
            return;
        };
        let first = DeviceId(RawId::new());
        let second = DeviceId(RawId::new());
        assert!(store.save_secret(&first, "phone", "first-secret").is_ok());
        assert!(
            store
                .save_secret(&second, "tablet", "second-secret")
                .is_ok()
        );
        assert!(store.delete_for(&first).is_ok());
        let missing = store.load_secret(&first);
        assert!(matches!(missing, Ok(None)));
        let kept = store.load_secret(&second);
        assert!(kept.is_ok(), "other device must survive the delete");
        let Ok(Some(secret)) = kept else {
            return;
        };
        assert_eq!(secret.bytes(), "second-secret".as_bytes());
        assert!(store.delete_for(&first).is_ok());
        assert!(store.delete_for(&DeviceId(RawId::new())).is_ok());
        let absent = temp.path().join("absent.json");
        let Some(absent_store) = open_device_auth_store(&absent) else {
            return;
        };
        assert!(absent_store.delete_for(&first).is_ok());
        assert!(!absent.exists(), "delete must not create the file");
    }

    #[test]
    fn device_auth_second_save_rotates_the_secret() {
        let Some(temp) = fresh_tempdir() else {
            return;
        };
        let path = temp.path().join("device-auth.json");
        let Some(store) = open_device_auth_store(&path) else {
            return;
        };
        let device = DeviceId(RawId::new());
        assert!(store.save_secret(&device, "phone", "first-secret").is_ok());
        assert!(store.save_secret(&device, "phone", "second-secret").is_ok());
        let loaded = store.load_secret(&device);
        assert!(loaded.is_ok(), "load must succeed");
        let Ok(Some(secret)) = loaded else {
            return;
        };
        assert_eq!(secret.bytes(), "second-secret".as_bytes());
    }

    #[test]
    fn device_auth_persists_across_store_instances() {
        let Some(temp) = fresh_tempdir() else {
            return;
        };
        let path = temp.path().join("device-auth.json");
        let device = DeviceId(RawId::new());
        let descriptor = "phone \"pro\"\nline2\t✓";
        let Some(first) = open_device_auth_store(&path) else {
            return;
        };
        assert!(
            first
                .save_secret(&device, descriptor, "pairing-secret-value")
                .is_ok()
        );
        drop(first);
        let Some(second) = open_device_auth_store(&path) else {
            return;
        };
        let loaded = second.load_secret(&device);
        assert!(loaded.is_ok(), "load must succeed");
        let Ok(Some(secret)) = loaded else {
            return;
        };
        assert_eq!(secret.bytes(), "pairing-secret-value".as_bytes());
    }

    #[test]
    fn device_auth_debug_carries_no_secret_or_descriptor() {
        let Some(temp) = fresh_tempdir() else {
            return;
        };
        let path = temp.path().join("device-auth.json");
        let Some(store) = open_device_auth_store(&path) else {
            return;
        };
        let marker = "marker-secret-9d3f41";
        let descriptor = "marker-descriptor-6be2";
        assert!(
            store
                .save_secret(&DeviceId(RawId::new()), descriptor, marker)
                .is_ok()
        );
        let rendered = format!("{store:?}");
        let marker_hex = super::encode_hex_lower(marker.as_bytes());
        assert!(rendered.contains("FileDeviceAuthStore"));
        assert!(rendered.contains("entries"));
        assert!(!rendered.contains(marker));
        assert!(!rendered.contains(marker_hex.as_str()));
        assert!(!rendered.contains(descriptor));
    }
}
