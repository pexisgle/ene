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

mod registration;

use std::collections::HashMap;
use std::sync::Mutex;

use ene_primitive::{RawId, WallClockWithTz};
use hmac::{Hmac, Mac};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub use registration::{
    CredentialIntentRepository, RegistrationApply, RegistrationFingerprint, RegistrationState,
};

/// Non-secret handle naming one stored credential.
///
/// Safe to clone, log, and persist: it carries provider and label only, never
/// key material. Fields are private so every ref passes [`CredentialRef::new`],
/// which enforces the same grammar as the management wire target
/// (`credential:{provider}:{label}`): the provider is non-blank and contains
/// no `:` separator; the label is non-empty and may contain `:`. The advisory
/// id is derived from those parts, so provider, label, and id can never
/// disagree, and two distinct `(provider, label)` pairs can never share an id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CredentialRef {
    /// Provider name; matched exactly.
    provider: String,
    /// Owner-chosen label distinguishing credentials of one provider.
    label: String,
}

/// Why a [`CredentialRef`] could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CredentialRefError {
    /// Provider was blank or contained the `:` separator.
    #[error("credential provider must be non-blank and contain no ':'")]
    InvalidProvider,
    /// Label was empty.
    #[error("credential label must be non-empty")]
    InvalidLabel,
}

impl CredentialRef {
    /// Builds a ref from its parts, enforcing the credential grammar.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialRefError::InvalidProvider`] for a blank provider or
    /// one containing `:`, and [`CredentialRefError::InvalidLabel`] for an
    /// empty label. A label may contain `:`, matching the wire parser.
    pub fn new(
        provider: impl Into<String>,
        label: impl Into<String>,
    ) -> Result<Self, CredentialRefError> {
        let provider = provider.into();
        let label = label.into();
        if provider.trim().is_empty() || provider.contains(':') {
            return Err(CredentialRefError::InvalidProvider);
        }
        if label.is_empty() {
            return Err(CredentialRefError::InvalidLabel);
        }
        Ok(Self { provider, label })
    }

    /// Provider name; matched exactly.
    #[must_use]
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Owner-chosen label distinguishing credentials of one provider.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Stable composite id of `provider:label`, not a secret.
    ///
    /// Derived, never stored separately: the id always reflects the validated
    /// parts above.
    #[must_use]
    pub fn id(&self) -> String {
        format!("{}:{}", self.provider, self.label)
    }
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
    /// The provider name was blank or contained `:`.
    InvalidProvider,
    /// The label was empty.
    InvalidLabel,
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
/// The provider and label must satisfy [`CredentialRef::new`]: a provider that
/// is blank or contains `:` yields [`RegisterOutcome::InvalidProvider`], and an
/// empty label yields [`RegisterOutcome::InvalidLabel`], both without touching
/// the repository. When a ref already exists for `(provider, label)`, the
/// stored ref is returned in [`RegisterOutcome::AlreadyExists`] and no write
/// occurs.
pub async fn register(
    cmd: RegisterCredentialCommand,
    repo: &impl CredentialRefRepository,
) -> Result<RegisterOutcome, CredentialTechnicalError> {
    let cred = match CredentialRef::new(cmd.provider, cmd.label) {
        Ok(cred) => cred,
        Err(CredentialRefError::InvalidProvider) => {
            return Ok(RegisterOutcome::InvalidProvider);
        }
        Err(CredentialRefError::InvalidLabel) => return Ok(RegisterOutcome::InvalidLabel),
    };
    if let Some(existing) = repo.load_ref(cred.provider(), cred.label()).await? {
        return Ok(RegisterOutcome::AlreadyExists(existing));
    }
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
    /// Entries keyed by the validated ref itself; values hold the confined
    /// secret.
    entries: Mutex<HashMap<CredentialRef, SecretValue>>,
}

impl core::fmt::Debug for MemoryCredentialStore {
    /// Renders the entry count and public refs; secrets are never rendered.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let refs: Vec<&CredentialRef> = entries.keys().collect();
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
        let mut entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.insert(cred, SecretValue::new(secret.as_bytes().to_vec()));
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
        let Some(secret) = entries.get(cred) else {
            let id = cred.id();
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("unknown credential {id}"),
            });
        };
        let Ok(bearer) = core::str::from_utf8(secret.bytes()) else {
            let id = cred.id();
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
        entries.remove(cred);
        Ok(())
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        let entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.contains_key(cred)
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
    /// Opaque wire projection of this device, minted fresh at approval and
    /// unrelated to [`DeviceId`]'s bytes: the only device string that ever
    /// crosses the wire. Clients echo it; they never derive or resolve it.
    pub wire: String,
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

    /// Loads the paired record for a wire projection, if any.
    ///
    /// The only durable wire-to-domain resolution: callers holding an
    /// opaque wire string (proof verification, sender attribution) resolve
    /// it here instead of parsing or deriving it.
    async fn find_device_by_wire(
        &self,
        wire: &str,
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
/// HMAC accepts keys of any length, so construction cannot fail; a
/// failure would break the primitive itself, and returning a fixed MAC
/// (for example zeros) would map that breakage onto a valid-looking proof.
#[expect(
    clippy::expect_used,
    reason = "HMAC-SHA256 accepts keys of any length; construction failure is an unreachable primitive invariant"
)]
fn compute_pairing_mac(secret: &str, nonce: &str) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC-SHA256 accepts keys of any length");
    mac.update(nonce.as_bytes());
    let digest = mac.finalize().into_bytes();
    let mut out = [0_u8; 32];
    out.copy_from_slice(&digest);
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
/// Name of the process environment variable carrying the `OpenAI` bearer.
///
/// The only environment input [`EnvCredentialStore`] ever reads, and only
/// inside [`CredentialStore::with_bearer`] and [`CredentialStore::contains`],
/// at call time.
pub const ENV_API_KEY: &str = "ENE_OPENAI_API_KEY";

/// Environment-backed bearer store for the `OpenAI` provider.
///
/// The struct is fieldless by design: the bearer is read from
/// [`ENV_API_KEY`] on every call and never cached in memory, so backup
/// exclusion holds trivially (there is nothing to back up) and key rotation
/// takes effect on the next call without a restart. Only the `"openai"`
/// provider is served (closed world until real OS stores arrive); every other
/// provider reports absent.
///
/// Request-builder discipline (shared with every [`CredentialStore`]): the
/// bearer is wrapped in [`SecretValue`] internally and lent as `&str` into
/// the caller's closure, which must build its owned request (headers, body)
/// there and perform I/O after it returns. Nothing borrows the key out, and
/// errors carry a status class only, never key material.
#[derive(Debug, Clone, Copy, Default)]
pub struct EnvCredentialStore;

impl EnvCredentialStore {
    /// Creates the store. Performs no I/O; the environment is read per call.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

// Pure-over-closure lookup shared by `contains` and `with_bearer`, so both
// agree on provider gating and emptiness. Production passes
// `|name| std::env::var(name).ok()` (a safe function; no `unsafe` involved)
// as `lookup`; tests inject closures, which keeps them hermetic: only the
// one-line wiring at each call site touches the real process environment. An
// empty value counts as absent, matching an unset variable.
fn resolve_for(provider: &str, lookup: impl FnOnce(&str) -> Option<String>) -> Option<SecretValue> {
    if provider != "openai" {
        return None;
    }
    let raw = lookup(ENV_API_KEY)?;
    if raw.is_empty() {
        return None;
    }
    Some(SecretValue::new(raw.into_bytes()))
}

impl CredentialStore for EnvCredentialStore {
    fn with_bearer<R>(
        &self,
        cred: &CredentialRef,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        // Single live read of the process environment per call: never cached,
        // rotation-friendly. `std::env::var` is a safe function.
        let Some(secret) = resolve_for(&cred.provider, |name| std::env::var(name).ok()) else {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: "env credential missing".to_owned(),
            });
        };
        // Defensive: values arriving via `std::env::var` are Unicode by
        // construction, but the store contract reports non-UTF-8 bearers.
        let Ok(bearer) = core::str::from_utf8(secret.bytes()) else {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: "env credential is not valid UTF-8".to_owned(),
            });
        };
        // The borrow of `bearer` cannot escape: `f` must build its owned
        // request inside this closure.
        Ok(f(bearer))
    }

    fn delete(&self, _cred: &CredentialRef) -> Result<(), CredentialTechnicalError> {
        // Nothing durable to remove: the bearer lives in the process
        // environment, not in this store. Revocation is ref-side, by removing
        // the `CredentialRef` from the `CredentialRefRepository`.
        Ok(())
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        // Same predicate as `with_bearer`'s gate, so availability and access
        // never disagree. Existence is non-secret metadata.
        resolve_for(&cred.provider, |name| std::env::var(name).ok()).is_some()
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
        let rendered = render_device_auth_file(entries)?;
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

// One file entry: secret bytes as lowercase hex plus display and timing
// metadata. Keys live in the surrounding map, canonicalized to hyphenated
// UUID text. Unknown fields are rejected at decode (`deny_unknown_fields`)
// so a hand-edited file with stray keys fails closed instead of silently
// dropping them; duplicate and missing fields are rejected by the derived
// `Deserialize` impl itself.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
// device key (`BTreeMap` iteration), no whitespace, trailing newline.
// Struct field order fixes the entry key order, so the rendering the doc
// comment on [`FileDeviceAuthStore`] shows is exact. `serde_json` is the one
// JSON implementation for both directions; a render failure is reported,
// never defaulted.
fn render_device_auth_file(
    entries: &BTreeMap<String, StoredDeviceAuth>,
) -> Result<String, CredentialTechnicalError> {
    #[derive(Serialize)]
    struct Document<'a> {
        devices: &'a BTreeMap<String, StoredDeviceAuth>,
    }
    let mut rendered = serde_json::to_string(&Document { devices: entries }).map_err(|_| {
        CredentialTechnicalError::StorageUnavailable {
            reason: String::from("device-auth entries cannot be rendered"),
        }
    })?;
    rendered.push('\n');
    Ok(rendered)
}

// Parses the whole document with `serde_json`; every failure maps to one
// fixed content-free detail. `serde_json` error displays can echo the
// offending input (unexpected values, unknown field names), and this file
// holds secrets and descriptors — so nothing from the decoder output ever
// reaches an error string. Semantic checks (UUID keys, hex secrets,
// RFC 3339 timestamps) happen in `read_entries`.
fn parse_device_auth_file(bytes: &[u8]) -> Result<BTreeMap<String, StoredDeviceAuth>, String> {
    serde_json::from_slice::<DeviceAuthFile>(bytes)
        .map(|file| file.devices)
        .map_err(|_| "file is not a valid device-auth document".to_owned())
}

// Whole-file document: exactly one `devices` section mapping canonical
// device UUID text to entries. Unknown top-level fields are rejected, so a
// hand-edited file with stray keys fails closed instead of silently
// dropping them.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceAuthFile {
    /// Device entries keyed by canonical UUID text; duplicates rejected.
    #[serde(deserialize_with = "devices_without_duplicates")]
    devices: BTreeMap<String, StoredDeviceAuth>,
}

// Rejects duplicate device entries at decode: deserializing straight into
// a map would let a later entry silently overwrite an earlier one, but the
// custody contract fails closed on hand-edited files instead.
fn devices_without_duplicates<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, StoredDeviceAuth>, D::Error>
where
    D: Deserializer<'de>,
{
    struct WithoutDuplicates;
    impl<'de> Visitor<'de> for WithoutDuplicates {
        type Value = BTreeMap<String, StoredDeviceAuth>;
        fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            formatter.write_str("a devices object without duplicate entries")
        }
        fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut devices = BTreeMap::new();
            while let Some((key, entry)) = access.next_entry::<String, StoredDeviceAuth>()? {
                if devices.insert(key, entry).is_some() {
                    return Err(serde::de::Error::custom("duplicate device entry"));
                }
            }
            Ok(devices)
        }
    }
    deserializer.deserialize_map(WithoutDuplicates)
}

/// One requested-but-not-yet-approved credential registration.
///
/// `provider` and `label` name the requested credential exactly as the future
/// [`CredentialRef`] would; they carry no secret material, so derived
/// [`core::fmt::Debug`] is safe. `requested_at` records when the request was
/// recorded, for display and audit only.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PendingCredentialApproval {
    /// Provider the requested credential belongs to.
    pub provider: String,
    /// Owner-chosen label distinguishing credentials of one provider.
    pub label: String,
    /// Wall-clock time with its creation offset recording when requested.
    pub requested_at: WallClockWithTz,
}

/// Persistence boundary for credential registration approvals.
///
/// Trust story: registration intent only proposes. A registration request
/// (for example, one arriving over the wire) calls `request_approval`, which
/// records a pending entry and never a usable credential. A Host-local
/// trusted inlet carrying explicit owner confirmation then calls
/// `approve_pending`, which flips the pending entry to usable. Availability
/// stays two-part: [`credential_availability`] still reports ref-usable
/// (repository) AND bearer-present (store); callers additionally gate on
/// `is_approved`, and that gating lives in Host composition, NOT here. This
/// crate provides the approval fact; it never combines it with availability
/// itself.
///
/// Revocation is explicitly deferred: Stage 2 thin scope provides no
/// remove/revoke method, so approvals only accumulate.
///
/// Blank-input contract: Host ingress validates that provider and label are
/// non-blank before calling. Implementations perform no validation
/// themselves beyond treating blank input as absent: when `provider` or
/// `label` is empty or whitespace-only, `request_approval` records nothing
/// and returns `Ok(false)`, `approve_pending` returns `Ok(false)`,
/// `is_approved` returns `Ok(false)`, and `list_pending` never yields blank
/// entries. A blank pair can therefore never become usable here; callers must
/// not rely on these methods to report validation errors.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CredentialApprovalRepository: Send + Sync {
    /// Records a credential registration approval request.
    ///
    /// Returns `Ok(true)` only when a new pending entry was recorded.
    /// Returns `Ok(false)` without touching stored entries when a pending
    /// entry already exists for `(provider, label)`, when the pair is already
    /// approved, or when either input is blank (empty or whitespace-only;
    /// Host ingress validates non-blank before calling, so this is a
    /// defensive backstop, never validation feedback).
    async fn request_approval(
        &self,
        provider: String,
        label: String,
    ) -> Result<bool, CredentialTechnicalError>;

    /// Approves the pending request for `(provider, label)`, marking it usable.
    ///
    /// On a known pending pair this moves the entry from pending to usable
    /// and returns `Ok(true)`. Re-approving an already-usable pair returns
    /// `Ok(true)` idempotently with no state change. An unknown pair yields
    /// `Ok(false)` — not an error; the caller maps that outcome to a
    /// clarification request. A blank `provider` or `label` is treated as
    /// absent and likewise yields `Ok(false)` without recording anything.
    ///
    /// Returns `bool` rather than `Option`, unlike
    /// [`DevicePairingRepository::approve_pending`], because there is no
    /// minted record or one-time secret to hand back: the approval fact
    /// itself is the whole result.
    ///
    /// Approval records an Owner decision transported from a trusted inlet;
    /// the repository never decides whether approval is allowed, it records
    /// the decision it was given.
    async fn approve_pending(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<bool, CredentialTechnicalError>;

    /// Reports whether `(provider, label)` is approved (usable).
    ///
    /// Returns `Ok(true)` only after approval; pending-only, unknown, and
    /// blank pairs all yield `Ok(false)`.
    async fn is_approved(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<bool, CredentialTechnicalError>;

    /// Lists all currently pending credential approval requests.
    async fn list_pending(
        &self,
    ) -> Result<Vec<PendingCredentialApproval>, CredentialTechnicalError>;
}

#[cfg(test)]
mod tests {
    use super::DeviceId;
    use super::FileDeviceAuthStore;
    use super::{
        CredentialRef, CredentialRefError, CredentialStore, CredentialTechnicalError,
        MemoryCredentialStore, RegisterCredentialCommand, RegisterOutcome, credential_availability,
        register,
    };
    use ene_primitive::RawId;
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
            refs.insert((cred.provider().to_owned(), cred.label().to_owned()), cred);
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

    fn acme_main() -> CredentialRef {
        CredentialRef::new("acme", "main").expect("valid test fixture")
    }

    #[tokio::test]
    async fn register_persists_a_new_ref() {
        let repo = FakeRepo::new();
        let outcome = register(command(), &repo).await;
        assert!(matches!(outcome, Ok(RegisterOutcome::Registered(_))));
        assert_eq!(repo.save_count().await, 1);
    }

    #[tokio::test]
    async fn register_rejects_grammar_violations_without_writing() {
        let repo = FakeRepo::new();
        for (provider, label, expected) in [
            ("   ", "main", RegisterOutcome::InvalidProvider),
            ("acme:bad", "main", RegisterOutcome::InvalidProvider),
            ("acme", "", RegisterOutcome::InvalidLabel),
        ] {
            let cmd = RegisterCredentialCommand {
                provider: provider.to_owned(),
                label: label.to_owned(),
            };
            let outcome = register(cmd, &repo)
                .await
                .expect("register answers an outcome");
            assert_eq!(outcome, expected, "pair {provider:?}:{label:?}");
        }
        assert_eq!(repo.save_count().await, 0);
    }

    #[tokio::test]
    async fn re_register_returns_the_existing_ref_without_overwriting() {
        let repo = FakeRepo::new();
        let first_outcome = register(command(), &repo).await;
        let RegisterOutcome::Registered(first) =
            first_outcome.expect("register succeeds with a valid command")
        else {
            panic!("a fresh provider:label registers");
        };
        let second_outcome = register(command(), &repo).await;
        let RegisterOutcome::AlreadyExists(existing) =
            second_outcome.expect("re-register answers an outcome")
        else {
            panic!("the same provider:label already exists");
        };
        assert_eq!(existing, first);
        assert_eq!(repo.save_count().await, 1);
    }

    #[test]
    fn credential_ref_grammar_is_fixed() {
        assert_eq!(acme_main().id(), "acme:main");
        assert_eq!(acme_main().provider(), "acme");
        assert_eq!(acme_main().label(), "main");
        // The id splits at the first ':', so a label may contain ':'.
        let colon_label = CredentialRef::new("acme", "team:main").expect("label may contain ':'");
        assert_eq!(colon_label.id(), "acme:team:main");
        assert_eq!(colon_label.label(), "team:main");
        assert_eq!(
            CredentialRef::new("acme:bad", "main"),
            Err(CredentialRefError::InvalidProvider)
        );
        assert_eq!(
            CredentialRef::new(" ", "main"),
            Err(CredentialRefError::InvalidProvider)
        );
        assert_eq!(
            CredentialRef::new("acme", ""),
            Err(CredentialRefError::InvalidLabel)
        );
    }

    #[test]
    fn availability_requires_both_registry_and_store() {
        let store = MemoryCredentialStore::new();
        let cred = acme_main();
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
        let cred = acme_main();
        store.insert(cred.clone(), "bearer-token");
        let seen = store.with_bearer(&cred, str::len);
        assert_eq!(seen, Ok("bearer-token".len()));
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

    fn fresh_tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir must be available")
    }

    fn open_device_auth_store(path: &std::path::Path) -> FileDeviceAuthStore {
        FileDeviceAuthStore::open(path).expect("device-auth store must open")
    }

    #[test]
    fn device_auth_roundtrip_preserves_secret_bytes() {
        let temp = fresh_tempdir();
        let path = temp.path().join("device-auth.json");
        let store = open_device_auth_store(&path);
        let device = DeviceId(RawId::new());
        let saved = store.save_secret(&device, "phone", "pairing-secret-value");
        assert!(saved.is_ok(), "save must succeed");
        let loaded = store.load_secret(&device);
        let secret = loaded.unwrap().unwrap();
        assert_eq!(secret.bytes(), "pairing-secret-value".as_bytes());
    }

    #[test]
    fn device_auth_missing_file_loads_none_and_missing_parent_fails_open() {
        let temp = fresh_tempdir();
        let path = temp.path().join("device-auth.json");
        let store = open_device_auth_store(&path);
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
            "{\"devices\":{\"123e4567-e89b-12d3-a456-426614174000\":\
             {\"secret_hex\":\"00\",\"descriptor\":\"d\",\
             \"paired_at\":\"2026-09-08T12:00:00+09:00\",\"unknown\":\"x\"}}}"
                .to_owned(),
            "{\"devices\":{\"123e4567-e89b-12d3-a456-426614174000\":\
             {\"secret_hex\":\"00\",\"secret_hex\":\"00\",\"descriptor\":\"d\",\
             \"paired_at\":\"2026-09-08T12:00:00+09:00\"}}}"
                .to_owned(),
            extra_field,
            keyed.clone() + "}]",
            duplicate_key,
        ];
        let temp = fresh_tempdir();
        let path = temp.path().join("device-auth.json");
        for fixture in fixtures {
            let written = std::fs::write(&path, &fixture);
            assert!(written.is_ok(), "fixture setup must succeed");
            let store = open_device_auth_store(&path);
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

    #[test]
    fn device_auth_file_renders_canonical_json() {
        let temp = fresh_tempdir();
        let path = temp.path().join("device-auth.json");
        let store = open_device_auth_store(&path);
        let first = DeviceId(RawId::new());
        let second = DeviceId(RawId::new());
        assert!(store.save_secret(&first, "phone", "first-secret").is_ok());
        assert!(
            store
                .save_secret(&second, "tablet", "second-secret")
                .is_ok()
        );
        let raw = std::fs::read(&path);
        let raw = raw.unwrap();
        assert!(
            raw.starts_with(b"{\"devices\":{"),
            "rendering keeps the single-section shape"
        );
        assert!(
            raw.ends_with(b"}}\n"),
            "rendering is compact with a trailing newline"
        );
        assert!(
            !raw.contains(&b' '),
            "rendering carries no whitespace padding"
        );
    }

    #[test]
    fn device_auth_reads_pre_serde_documents() {
        // Same document shape as earlier releases (field order and escape
        // sequences): existing custody files must keep parsing.
        let fixture = "{\"devices\":{\"123e4567-e89b-12d3-a456-426614174000\":\
            {\"secret_hex\":\"00\",\"descriptor\":\"a\\\"b\\\\nc✓\",\
            \"paired_at\":\"2026-09-08T12:00:00+09:00\"}}}\n";
        let temp = fresh_tempdir();
        let path = temp.path().join("device-auth.json");
        let written = std::fs::write(&path, fixture);
        assert!(written.is_ok(), "fixture setup must succeed");
        let store = open_device_auth_store(&path);
        let device = super::parse_device_key("123e4567-e89b-12d3-a456-426614174000").unwrap();
        let loaded = store.load_secret(&device);
        let secret = loaded.unwrap().unwrap();
        assert_eq!(secret.bytes(), &[0x00]);
    }

    #[cfg(unix)]
    #[test]
    fn device_auth_open_tightens_lax_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let temp = fresh_tempdir();
        let path = temp.path().join("device-auth.json");
        let written = std::fs::write(&path, "{\"devices\":{}}");
        assert!(written.is_ok(), "fixture setup must succeed");
        let lax = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
        assert!(lax.is_ok(), "fixture setup must succeed");
        let store = open_device_auth_store(&path);
        let meta = std::fs::metadata(&path);
        let meta = meta.unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        let loaded = store.load_secret(&DeviceId(RawId::new()));
        assert!(matches!(loaded, Ok(None)));
    }

    #[cfg(unix)]
    #[test]
    fn device_auth_saved_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let temp = fresh_tempdir();
        let path = temp.path().join("device-auth.json");
        let store = open_device_auth_store(&path);
        let saved = store.save_secret(&DeviceId(RawId::new()), "phone", "pairing-secret");
        assert!(saved.is_ok(), "save must succeed");
        let meta = std::fs::metadata(&path);
        let meta = meta.unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn device_auth_delete_removes_only_the_target() {
        let temp = fresh_tempdir();
        let path = temp.path().join("device-auth.json");
        let store = open_device_auth_store(&path);
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
        let secret = kept.unwrap().unwrap();
        assert_eq!(secret.bytes(), "second-secret".as_bytes());
        assert!(store.delete_for(&first).is_ok());
        assert!(store.delete_for(&DeviceId(RawId::new())).is_ok());
        let absent = temp.path().join("absent.json");
        let absent_store = open_device_auth_store(&absent);
        assert!(absent_store.delete_for(&first).is_ok());
        assert!(!absent.exists(), "delete must not create the file");
    }

    #[test]
    fn device_auth_second_save_rotates_the_secret() {
        let temp = fresh_tempdir();
        let path = temp.path().join("device-auth.json");
        let store = open_device_auth_store(&path);
        let device = DeviceId(RawId::new());
        assert!(store.save_secret(&device, "phone", "first-secret").is_ok());
        assert!(store.save_secret(&device, "phone", "second-secret").is_ok());
        let loaded = store.load_secret(&device);
        let secret = loaded.unwrap().unwrap();
        assert_eq!(secret.bytes(), "second-secret".as_bytes());
    }

    #[test]
    fn device_auth_persists_across_store_instances() {
        let temp = fresh_tempdir();
        let path = temp.path().join("device-auth.json");
        let device = DeviceId(RawId::new());
        let descriptor = "phone \"pro\"\nline2\t✓";
        let first = open_device_auth_store(&path);
        assert!(
            first
                .save_secret(&device, descriptor, "pairing-secret-value")
                .is_ok()
        );
        drop(first);
        let second = open_device_auth_store(&path);
        let loaded = second.load_secret(&device);
        let secret = loaded.unwrap().unwrap();
        assert_eq!(secret.bytes(), "pairing-secret-value".as_bytes());
    }

    #[test]
    fn device_auth_debug_carries_no_secret_or_descriptor() {
        let temp = fresh_tempdir();
        let path = temp.path().join("device-auth.json");
        let store = open_device_auth_store(&path);
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

#[cfg(test)]
mod env_credential_store_tests {
    use super::{
        CredentialRef, CredentialStore, CredentialTechnicalError, ENV_API_KEY, EnvCredentialStore,
        resolve_for,
    };
    use std::cell::Cell;

    fn openai_cred() -> CredentialRef {
        CredentialRef {
            id: "openai:main".to_owned(),
            provider: "openai".to_owned(),
            label: "main".to_owned(),
        }
    }

    fn other_cred() -> CredentialRef {
        CredentialRef {
            id: "acme:main".to_owned(),
            provider: "acme".to_owned(),
            label: "main".to_owned(),
        }
    }

    #[test]
    fn env_var_name_is_pinned() {
        assert_eq!(ENV_API_KEY, "ENE_OPENAI_API_KEY");
    }

    #[test]
    fn constructors_create_a_fieldless_store() {
        fn assert_default<T: Default>() {}
        assert_default::<EnvCredentialStore>();
        let via_new = EnvCredentialStore::new();
        assert!(!via_new.contains(&other_cred()));
        assert!(!EnvCredentialStore.contains(&other_cred()));
    }

    #[test]
    fn lookup_gates_on_provider_before_reading_env() {
        let calls = Cell::new(0_u32);
        let resolved = resolve_for("acme", |_| {
            calls.set(calls.get() + 1);
            Some("test-key".to_owned())
        });
        assert!(resolved.is_none());
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn lookup_accepts_a_present_non_empty_value() {
        let resolved = resolve_for("openai", |name| {
            assert_eq!(name, ENV_API_KEY);
            Some("test-key".to_owned())
        });
        assert!(resolved.is_some());
        let Some(secret) = resolved else {
            return;
        };
        assert!(matches!(
            core::str::from_utf8(secret.bytes()),
            Ok("test-key")
        ));
    }

    #[test]
    fn resolved_bearer_builds_an_owned_request() {
        let resolved = resolve_for("openai", |_| Some("test-key".to_owned()));
        assert!(resolved.is_some());
        let Some(secret) = resolved else {
            return;
        };
        let Ok(bearer) = core::str::from_utf8(secret.bytes()) else {
            return;
        };
        let authorization = format!("Bearer {bearer}");
        drop(secret);
        assert_eq!(authorization, "Bearer test-key");
    }

    #[test]
    fn lookup_treats_a_missing_value_as_absent() {
        let resolved = resolve_for("openai", |_| None);
        assert!(resolved.is_none());
    }

    #[test]
    fn lookup_treats_an_empty_value_as_absent() {
        let resolved = resolve_for("openai", |_| Some(String::new()));
        assert!(resolved.is_none());
    }

    #[test]
    fn store_reports_other_providers_absent_without_reading_env() {
        let store = EnvCredentialStore;
        assert!(!store.contains(&other_cred()));
    }

    #[test]
    fn store_with_bearer_rejects_other_providers() {
        let store = EnvCredentialStore;
        let outcome = store.with_bearer(&other_cred(), str::len);
        assert!(outcome.is_err());
        let Err(CredentialTechnicalError::StorageUnavailable { reason }) = outcome else {
            return;
        };
        assert_eq!(reason, "env credential missing");
    }

    #[test]
    fn delete_reports_success_with_nothing_durable() {
        let store = EnvCredentialStore;
        assert!(store.delete(&openai_cred()).is_ok());
        assert!(store.delete(&other_cred()).is_ok());
    }

    #[test]
    fn debug_rendering_names_the_store_only() {
        let store = EnvCredentialStore;
        let rendered = format!("{store:?}");
        assert_eq!(rendered, "EnvCredentialStore");
    }
}
