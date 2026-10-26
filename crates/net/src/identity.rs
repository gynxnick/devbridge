//! Long-lived device identity: an Ed25519 key pair and self-signed
//! certificate. Both live in the OS credential store (macOS Keychain, Windows
//! Credential Manager); nothing secret is written to the database or logs.

use std::collections::HashMap;
use std::sync::Mutex;

use data_encoding::{BASE64, HEXLOWER};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("the system credential store is unavailable: {0}")]
    Store(String),
    #[error("could not create device certificate: {0}")]
    Cert(String),
    #[error("stored identity is corrupt")]
    Corrupt,
}

/// Abstraction over the OS credential store so tests can use memory.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, IdentityError>;
    fn set(&self, key: &str, value: &[u8]) -> Result<(), IdentityError>;
    fn delete(&self, key: &str) -> Result<(), IdentityError>;
}

/// OS credential store via the `keyring` crate.
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    /// `service` distinguishes installations (e.g. includes a data-dir hash).
    pub fn new(service: impl Into<String>) -> Self {
        Self { service: service.into() }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry, IdentityError> {
        keyring::Entry::new(&self.service, key).map_err(|e| IdentityError::Store(e.to_string()))
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, IdentityError> {
        // Stored base64-encoded: some backends only accept text reliably.
        match self.entry(key)?.get_password() {
            Ok(s) => BASE64.decode(s.as_bytes()).map(Some).map_err(|_| IdentityError::Corrupt),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(IdentityError::Store(e.to_string())),
        }
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), IdentityError> {
        self.entry(key)?.set_password(&BASE64.encode(value)).map_err(|e| IdentityError::Store(e.to_string()))
    }
    fn delete(&self, key: &str) -> Result<(), IdentityError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(IdentityError::Store(e.to_string())),
        }
    }
}

/// In-memory store for tests and ephemeral instances.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, Vec<u8>>>);

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, IdentityError> {
        Ok(self.0.lock().unwrap_or_else(|p| p.into_inner()).get(key).cloned())
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), IdentityError> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).insert(key.into(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), IdentityError> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).remove(key);
        Ok(())
    }
}

const KEY_ENTRY: &str = "device-key";
const CERT_ENTRY: &str = "device-cert";

#[derive(Clone)]
pub struct Identity {
    pub cert_der: Vec<u8>,
    key_der: Vec<u8>,
    pub fingerprint: String,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity").field("fingerprint", &self.fingerprint).finish_non_exhaustive()
    }
}

impl Identity {
    pub fn generate() -> Result<Self, IdentityError> {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).map_err(|e| IdentityError::Cert(e.to_string()))?;
        let mut params = rcgen::CertificateParams::new(vec!["devbridge.local".to_string()])
            .map_err(|e| IdentityError::Cert(e.to_string()))?;
        // Long validity: trust comes from pinning, not from expiry.
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2124, 1, 1);
        let cert = params.self_signed(&key).map_err(|e| IdentityError::Cert(e.to_string()))?;
        let cert_der = cert.der().to_vec();
        Ok(Self { fingerprint: fingerprint(&cert_der), cert_der, key_der: key.serialize_der() })
    }

    /// Loads the identity from the store, creating and saving one on first run.
    pub fn load_or_create(store: &dyn SecretStore) -> Result<Self, IdentityError> {
        if let (Some(key_der), Some(cert_der)) = (store.get(KEY_ENTRY)?, store.get(CERT_ENTRY)?) {
            return Ok(Self { fingerprint: fingerprint(&cert_der), cert_der, key_der });
        }
        let id = Self::generate()?;
        store.set(KEY_ENTRY, &id.key_der)?;
        store.set(CERT_ENTRY, &id.cert_der)?;
        Ok(id)
    }

    /// Deletes the stored identity (used by "reset device identity").
    pub fn forget(store: &dyn SecretStore) -> Result<(), IdentityError> {
        store.delete(KEY_ENTRY)?;
        store.delete(CERT_ENTRY)
    }

    pub(crate) fn key_der(&self) -> &[u8] {
        &self.key_der
    }
}

/// Lowercase hex SHA-256 of a certificate's DER encoding.
pub fn fingerprint(cert_der: &[u8]) -> String {
    HEXLOWER.encode(ring::digest::digest(&ring::digest::SHA256, cert_der).as_ref())
}

/// Short, human-comparable form: `ab12 cd34 ef56 7890`.
pub fn short_fingerprint(fp: &str) -> String {
    fp.as_bytes().chunks(4).take(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_then_load_is_stable() {
        let store = MemoryStore::default();
        let a = Identity::load_or_create(&store).unwrap();
        let b = Identity::load_or_create(&store).unwrap();
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.fingerprint.len(), 64);
        Identity::forget(&store).unwrap();
        let c = Identity::load_or_create(&store).unwrap();
        assert_ne!(a.fingerprint, c.fingerprint);
        assert_eq!(short_fingerprint("0123456789abcdef0000"), "0123 4567 89ab cdef");
        assert!(!format!("{a:?}").contains("key_der: ["), "debug output must not dump the key");
    }
}
