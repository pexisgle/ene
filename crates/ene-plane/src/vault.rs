use blake3::Hash;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;
use zeroize::Zeroize;

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
}

/// Host-only handle. Plugins never receive the secret bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InjectRef {
    pub credential_id: String,
}

/// Passphrase-derived file vault (P-907). Plaintext never leaves the host.
pub struct Vault {
    path: PathBuf,
    key: Mutex<[u8; 32]>,
}

impl Vault {
    pub fn open_file(path: impl AsRef<Path>, passphrase: &str) -> Result<Self, VaultError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let key = derive_key(passphrase);
        let vault = Self {
            path,
            key: Mutex::new(key),
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
        self.key.lock().zeroize();
    }
}

impl Vault {
    fn load(&self) -> Result<HashMap<String, Vec<u8>>, VaultError> {
        let sealed = std::fs::read(&self.path)?;
        let key = *self.key.lock();
        let plain = open(&key, &sealed)?;
        serde_json::from_slice(&plain).map_err(|err| VaultError::Codec(err.to_string()))
    }

    fn save(&self, map: &HashMap<String, Vec<u8>>) -> Result<(), VaultError> {
        let plain = serde_json::to_vec(map).map_err(|err| VaultError::Codec(err.to_string()))?;
        let key = *self.key.lock();
        let sealed = seal(&key, &plain)?;
        let mut file = std::fs::File::create(&self.path)?;
        file.write_all(&sealed)?;
        Ok(())
    }
}

fn derive_key(passphrase: &str) -> [u8; 32] {
    blake3::derive_key("ene-vault/v1", passphrase.as_bytes())
}

fn random_nonce() -> Result<[u8; 16], VaultError> {
    let mut buf = [0_u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf)
}

fn seal(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, VaultError> {
    let nonce = random_nonce()?;
    let mut hasher = blake3::Hasher::new_derive_key("ene-vault/stream");
    hasher.update(key);
    hasher.update(&nonce);
    let mut keystream = vec![0_u8; plaintext.len()];
    hasher.finalize_xof().fill(&mut keystream);
    let mut ct = plaintext.to_vec();
    for (dst, src) in ct.iter_mut().zip(keystream.iter()) {
        *dst ^= src;
    }
    let mut mac = blake3::Hasher::new_derive_key("ene-vault/mac");
    mac.update(key);
    mac.update(&nonce);
    mac.update(&ct);
    let tag: Hash = mac.finalize();
    let mut out = Vec::with_capacity(16 + ct.len() + 32);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out.extend_from_slice(tag.as_bytes());
    Ok(out)
}

fn open(key: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>, VaultError> {
    if sealed.len() < 48 {
        return Err(VaultError::Integrity);
    }
    let nonce = &sealed[..16];
    let tag = &sealed[sealed.len() - 32..];
    let ct = &sealed[16..sealed.len() - 32];
    let mut mac = blake3::Hasher::new_derive_key("ene-vault/mac");
    mac.update(key);
    mac.update(nonce);
    mac.update(ct);
    if mac.finalize().as_bytes() != tag {
        return Err(VaultError::Integrity);
    }
    let mut hasher = blake3::Hasher::new_derive_key("ene-vault/stream");
    hasher.update(key);
    hasher.update(nonce);
    let mut keystream = vec![0_u8; ct.len()];
    hasher.finalize_xof().fill(&mut keystream);
    let mut plain = ct.to_vec();
    for (dst, src) in plain.iter_mut().zip(keystream.iter()) {
        *dst ^= src;
    }
    Ok(plain)
}
