use blake3::Hash;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;
use zeroize::Zeroize;

const MAGIC: &[u8; 4] = b"ENV1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 16;
const TAG_LEN: usize = 32;

/// Vault failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum VaultError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("unknown credential {0}")]
    Unknown(String),
    #[error("integrity check failed")]
    Integrity,
    #[error("codec: {0}")]
    Codec(String),
    #[error("passphrase must not be empty")]
    EmptyPassphrase,
}

/// Host-only handle. Plugins never receive the secret bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InjectRef {
    pub credential_id: String,
}

/// Passphrase-derived file vault (P-907). Plaintext never leaves the host.
pub struct Vault {
    path: PathBuf,
    passphrase: Mutex<Vec<u8>>,
}

impl Vault {
    pub fn open_file(path: impl AsRef<Path>, passphrase: &str) -> Result<Self, VaultError> {
        if passphrase.is_empty() {
            return Err(VaultError::EmptyPassphrase);
        }
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let vault = Self {
            path,
            passphrase: Mutex::new(passphrase.as_bytes().to_vec()),
        };
        if vault.path.exists() {
            drop(vault.load()?);
        } else {
            vault.save(&HashMap::new())?;
        }
        Ok(vault)
    }

    /// Open a vault using passphrase bytes from `key_path`. Creates the key
    /// file with 32 random bytes and mode `0600` when it is missing.
    pub fn open_or_create_keyfile(
        vault_path: impl AsRef<Path>,
        key_path: impl AsRef<Path>,
    ) -> Result<Self, VaultError> {
        let key_path = key_path.as_ref();
        let passphrase = read_or_create_keyfile(key_path)?;
        if passphrase.is_empty() {
            return Err(VaultError::EmptyPassphrase);
        }
        let path = vault_path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let vault = Self {
            path,
            passphrase: Mutex::new(passphrase),
        };
        if vault.path.exists() {
            drop(vault.load()?);
        } else {
            vault.save(&HashMap::new())?;
        }
        Ok(vault)
    }

    pub fn put(&self, id: &str, secret: &[u8]) -> Result<InjectRef, VaultError> {
        let mut map = self.load()?;
        map.insert(id.to_owned(), secret.to_vec());
        self.save(&map)?;
        Ok(InjectRef {
            credential_id: id.to_owned(),
        })
    }

    #[must_use]
    pub fn inject_ref(id: impl Into<String>) -> InjectRef {
        InjectRef {
            credential_id: id.into(),
        }
    }

    pub fn inject(&self, inject: &InjectRef) -> Result<Vec<u8>, VaultError> {
        let map = self.load()?;
        map.get(&inject.credential_id)
            .cloned()
            .ok_or_else(|| VaultError::Unknown(inject.credential_id.clone()))
    }

    /// Export is the exception path (providers/MCP). Callers must have approval.
    pub fn export(&self, id: &str) -> Result<Vec<u8>, VaultError> {
        self.inject(&InjectRef {
            credential_id: id.to_owned(),
        })
    }
}

impl Drop for Vault {
    fn drop(&mut self) {
        self.passphrase.lock().zeroize();
    }
}

impl Vault {
    fn load(&self) -> Result<HashMap<String, Vec<u8>>, VaultError> {
        let sealed = std::fs::read(&self.path)?;
        let mut passphrase = self.passphrase.lock().clone();
        let mut plain = open(&passphrase, &sealed)?;
        passphrase.zeroize();
        let parsed =
            serde_json::from_slice(&plain).map_err(|err| VaultError::Codec(err.to_string()));
        plain.zeroize();
        parsed
    }

    fn save(&self, map: &HashMap<String, Vec<u8>>) -> Result<(), VaultError> {
        let mut plain =
            serde_json::to_vec(map).map_err(|err| VaultError::Codec(err.to_string()))?;
        let mut passphrase = self.passphrase.lock().clone();
        let sealed = seal(&passphrase, &plain)?;
        passphrase.zeroize();
        plain.zeroize();
        let tmp = self.path.with_extension("tmp");
        write_secret_file(&tmp, &sealed)?;
        std::fs::rename(&tmp, &self.path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

fn read_or_create_keyfile(path: &Path) -> Result<Vec<u8>, VaultError> {
    if path.exists() {
        return std::fs::read(path).map_err(VaultError::from);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut bytes = [0_u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    write_secret_file(path, &bytes)?;
    Ok(bytes.to_vec())
}

fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)?;
    }
    Ok(())
}

fn derive_stream_key(passphrase: &[u8], salt: &[u8; SALT_LEN]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(passphrase);
    hasher.update(salt);
    *hasher.finalize().as_bytes()
}

fn random_bytes(len: usize) -> Result<Vec<u8>, VaultError> {
    let mut buf = vec![0_u8; len];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf)
}

fn seal(passphrase: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, VaultError> {
    let salt: [u8; SALT_LEN] = random_bytes(SALT_LEN)?
        .try_into()
        .map_err(|_| VaultError::Codec("invalid salt length".to_owned()))?;
    let nonce: [u8; NONCE_LEN] = random_bytes(NONCE_LEN)?
        .try_into()
        .map_err(|_| VaultError::Codec("invalid nonce length".to_owned()))?;
    let mut stream_key = derive_stream_key(passphrase, &salt);
    let mut hasher = blake3::Hasher::new_derive_key("ene-vault/stream");
    hasher.update(&stream_key);
    hasher.update(&nonce);
    let mut keystream = vec![0_u8; plaintext.len()];
    hasher.finalize_xof().fill(&mut keystream);
    let mut ct = plaintext.to_vec();
    for (dst, src) in ct.iter_mut().zip(keystream.iter()) {
        *dst ^= src;
    }
    keystream.zeroize();
    let mut mac = blake3::Hasher::new_derive_key("ene-vault/mac");
    mac.update(&stream_key);
    mac.update(&nonce);
    mac.update(&ct);
    let tag: Hash = mac.finalize();
    stream_key.zeroize();
    let mut out = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN + ct.len() + TAG_LEN);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out.extend_from_slice(tag.as_bytes());
    Ok(out)
}

fn open(passphrase: &[u8], sealed: &[u8]) -> Result<Vec<u8>, VaultError> {
    if sealed.len() < MAGIC.len() + SALT_LEN + NONCE_LEN + TAG_LEN {
        return Err(VaultError::Integrity);
    }
    if &sealed[..MAGIC.len()] != MAGIC {
        return Err(VaultError::Integrity);
    }
    let salt = &sealed[MAGIC.len()..MAGIC.len() + SALT_LEN];
    let nonce = &sealed[MAGIC.len() + SALT_LEN..MAGIC.len() + SALT_LEN + NONCE_LEN];
    let tag = &sealed[sealed.len() - TAG_LEN..];
    let ct = &sealed[MAGIC.len() + SALT_LEN + NONCE_LEN..sealed.len() - TAG_LEN];
    let salt_arr: [u8; SALT_LEN] = salt.try_into().map_err(|_| VaultError::Integrity)?;
    let mut stream_key = derive_stream_key(passphrase, &salt_arr);
    let mut mac = blake3::Hasher::new_derive_key("ene-vault/mac");
    mac.update(&stream_key);
    mac.update(nonce);
    mac.update(ct);
    if !constant_time_eq(mac.finalize().as_bytes(), tag) {
        stream_key.zeroize();
        return Err(VaultError::Integrity);
    }
    let mut hasher = blake3::Hasher::new_derive_key("ene-vault/stream");
    hasher.update(&stream_key);
    hasher.update(nonce);
    let mut keystream = vec![0_u8; ct.len()];
    hasher.finalize_xof().fill(&mut keystream);
    let mut plain = ct.to_vec();
    for (dst, src) in plain.iter_mut().zip(keystream.iter()) {
        *dst ^= src;
    }
    keystream.zeroize();
    stream_key.zeroize();
    Ok(plain)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}
