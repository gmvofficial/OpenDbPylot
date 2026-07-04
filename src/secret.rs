//! Secret vault for API keys.
//!
//! Default: `EncryptedFileSecretStore` — AES-256-GCM encrypted file, key derived
//! from your machine ID via Argon2id. Zero prompts, works everywhere.
//!
//! Optional fallback: `KeyringSecretStore` (macOS Keychain) enabled with the
//! `keychain` feature flag and `OPENDBPYLOT_SECRETS=keyring` env var.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use anyhow::{Context, Result};
use argon2::Argon2;
use rand::RngCore;

// ── Trait ────────────────────────────────────────────────────────────────────

pub trait SecretStore: Send + Sync {
    fn set(&self, key: &str, value: &str) -> Result<()>;
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn delete(&self, key: &str) -> Result<()>;
}

// ── Encrypted file vault (default) ───────────────────────────────────────────

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

/// AES-256-GCM encrypted secrets file.
///
/// The encryption key is derived from the machine's hardware UUID (macOS) or
/// `/etc/machine-id` (Linux) via Argon2id — no password prompt, no keychain,
/// no daemon required. The file is useless on any other machine.
pub struct EncryptedFileSecretStore {
    path: PathBuf,
    key: [u8; 32],
    salt: [u8; SALT_LEN],
    cache: Mutex<HashMap<String, String>>,
}

impl EncryptedFileSecretStore {
    /// Open (or create) the vault at `path`. Reads the machine ID automatically.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let machine_id = machine_id()?;

        if !path.exists() {
            // First run: generate a fresh salt and an empty vault.
            let mut salt = [0u8; SALT_LEN];
            OsRng.fill_bytes(&mut salt);
            let key = derive_key(&machine_id, &salt)?;
            let store = Self { path, key, salt, cache: Mutex::new(HashMap::new()) };
            store.flush(&store.cache.lock().unwrap())?;
            return Ok(store);
        }

        // Existing vault: read salt from the file header and decrypt.
        let raw = std::fs::read(&path)
            .with_context(|| format!("cannot read secrets file {}", path.display()))?;
        if raw.len() < SALT_LEN + NONCE_LEN + 1 {
            anyhow::bail!("secrets file is too short — it may be corrupted");
        }
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&raw[..SALT_LEN]);
        let key = derive_key(&machine_id, &salt)?;
        let map = decrypt_map(&raw, &key)?;

        Ok(Self { path, key, salt, cache: Mutex::new(map) })
    }

    fn flush(&self, map: &HashMap<String, String>) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let encrypted = encrypt_map(map, &self.key, &self.salt)?;
        std::fs::write(&self.path, encrypted)
            .with_context(|| format!("cannot write secrets file {}", self.path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

impl SecretStore for EncryptedFileSecretStore {
    fn set(&self, key: &str, value: &str) -> Result<()> {
        let mut g = self.cache.lock().unwrap();
        g.insert(key.to_string(), value.to_string());
        self.flush(&g)
    }

    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.cache.lock().unwrap().get(key).cloned())
    }

    fn delete(&self, key: &str) -> Result<()> {
        let mut g = self.cache.lock().unwrap();
        g.remove(key);
        self.flush(&g)
    }
}

// ── Encryption helpers ────────────────────────────────────────────────────────

fn derive_key(machine_id: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(machine_id.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow::anyhow!("key derivation failed: {e}"))?;
    Ok(key)
}

/// Encrypt a `HashMap<String,String>` → salt (16) + nonce (12) + ciphertext.
fn encrypt_map(map: &HashMap<String, String>, key: &[u8; 32], salt: &[u8; SALT_LEN]) -> Result<Vec<u8>> {
    let plaintext = serde_json::to_vec(map)?;
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| anyhow::anyhow!("cipher init failed: {e}"))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_ref())
        .map_err(|e| anyhow::anyhow!("encryption failed: {e}"))?;
    let mut out = Vec::with_capacity(SALT_LEN + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt_map(raw: &[u8], key: &[u8; 32]) -> Result<HashMap<String, String>> {
    let nonce_bytes = &raw[SALT_LEN..SALT_LEN + NONCE_LEN];
    let ciphertext = &raw[SALT_LEN + NONCE_LEN..];
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| anyhow::anyhow!("cipher init failed: {e}"))?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|_| anyhow::anyhow!(
            "failed to decrypt secrets — did you copy the file from a different machine?"
        ))?;
    Ok(serde_json::from_slice(&plaintext)?)
}

// ── Machine ID ────────────────────────────────────────────────────────────────

fn machine_id() -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
            .context("failed to read macOS machine ID")?;
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if line.contains("IOPlatformUUID") {
                if let Some(uuid) = line.split('"').nth(3) {
                    return Ok(uuid.to_string());
                }
            }
        }
        // Fallback to hostname if ioreg parsing fails
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(id) = std::fs::read_to_string("/etc/machine-id") {
            let id = id.trim().to_string();
            if !id.is_empty() {
                return Ok(id);
            }
        }
    }

    // Universal fallback
    let hostname = std::process::Command::new("hostname")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "opendbpylot-default".to_string());
    Ok(hostname)
}

// ── OS Keychain (optional fallback) ──────────────────────────────────────────

#[cfg(feature = "keychain")]
pub struct KeyringSecretStore {
    service: String,
}

#[cfg(feature = "keychain")]
impl KeyringSecretStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self { service: service.into() }
    }
}

#[cfg(feature = "keychain")]
impl SecretStore for KeyringSecretStore {
    fn set(&self, key: &str, value: &str) -> Result<()> {
        keyring::Entry::new(&self.service, key)?.set_password(value)?;
        Ok(())
    }
    fn get(&self, key: &str) -> Result<Option<String>> {
        match keyring::Entry::new(&self.service, key)?.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    fn delete(&self, key: &str) -> Result<()> {
        match keyring::Entry::new(&self.service, key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Plain JSON file fallback (useful in CI / headless environments).
pub struct FileSecretStore {
    path: PathBuf,
    cache: Mutex<HashMap<String, String>>,
}

impl FileSecretStore {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let cache: HashMap<String, String> = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            HashMap::new()
        };
        Ok(Self { path, cache: Mutex::new(cache) })
    }

    fn save(&self, map: &HashMap<String, String>) -> Result<()> {
        std::fs::write(&self.path, serde_json::to_vec_pretty(map)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

impl SecretStore for FileSecretStore {
    fn set(&self, key: &str, value: &str) -> Result<()> {
        let mut g = self.cache.lock().unwrap();
        g.insert(key.to_string(), value.to_string());
        self.save(&g)
    }
    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.cache.lock().unwrap().get(key).cloned())
    }
    fn delete(&self, key: &str) -> Result<()> {
        let mut g = self.cache.lock().unwrap();
        g.remove(key);
        self.save(&g)
    }
}
