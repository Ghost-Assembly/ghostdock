//! Secrets at rest.
//!
//! XChaCha20-Poly1305 from RustCrypto. Not hand-rolled, and not a home-made
//! construction over a primitive: an AEAD with a 192-bit nonce means a
//! randomly generated nonce per value is safe without tracking a counter,
//! which is the part that usually goes wrong.
//!
//! Every value is bound to a purpose string as associated data, so a sealed
//! Git token cannot be moved into an environment-variable column and still
//! decrypt. Without that, the database's own structure is unauthenticated.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroize as _;

/// Prefix on every sealed value, so the format can change later without
/// guessing at what an old row contains.
const VERSION: &str = "v1";

/// Purpose strings. Anything sealed under one cannot be opened under another.
pub mod purpose {
    /// A credential used to reach a Git remote.
    pub const GIT_CREDENTIAL: &str = "git-credential";
    /// A stack's environment variable value.
    pub const STACK_ENV: &str = "stack-env";
    /// Where an alert channel sends: webhook URLs carry their secret.
    pub const ALERT_URL: &str = "alert-channel-url";
    /// The bearer token an alert channel sends with.
    pub const ALERT_TOKEN: &str = "alert-channel-token";
}

/// Environment variable the server reads the key from.
///
/// Named here for error messages only. Reading it is the caller's job: this
/// crate takes the value as an argument, which keeps configuration out of the
/// persistence layer and makes the behaviour testable without mutating the
/// process environment.
pub const KEY_ENV: &str = "GHOSTDOCK_SECRET_KEY";

/// Name of the generated key file inside the data directory.
pub const KEY_FILE: &str = "secret.key";

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("could not decrypt: wrong key, wrong purpose, or the value was altered")]
    Undecryptable,
    #[error("stored secret is not in a format GhostDock understands")]
    Malformed,
    #[error("{KEY_ENV} must be 32 bytes encoded as base64")]
    BadKeyFromEnv,
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

pub type SecretResult<T> = std::result::Result<T, SecretError>;

/// Seals and opens stored secrets.
#[derive(Clone)]
pub struct Cipher {
    inner: chacha20poly1305::XChaCha20Poly1305,
}

impl std::fmt::Debug for Cipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material, not even as a placeholder that might
        // later be replaced with something real.
        f.write_str("Cipher(..)")
    }
}

impl Cipher {
    /// Builds a cipher from raw key bytes.
    #[must_use]
    pub fn new(key: &[u8; 32]) -> Self {
        use chacha20poly1305::KeyInit;
        Self {
            inner: XChaCha20Poly1305::new(key.into()),
        }
    }

    /// Uses `key_from_env` when supplied, otherwise a key file in
    /// `data_dir`, generating one on first run.
    ///
    /// A supplied key that cannot be parsed is an error rather than a reason
    /// to fall back: silently generating one would encrypt everything with a
    /// key the operator did not choose and cannot reproduce.
    pub fn load_or_create(data_dir: &Path, key_from_env: Option<&str>) -> SecretResult<Self> {
        if let Some(encoded) = key_from_env {
            let mut key = decode_key(encoded.trim()).ok_or(SecretError::BadKeyFromEnv)?;
            let cipher = Self::new(&key);
            key.zeroize();
            return Ok(cipher);
        }

        let path = key_path(data_dir);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let mut key: [u8; 32] = bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| SecretError::Malformed)?;
                let cipher = Self::new(&key);
                key.zeroize();
                Ok(cipher)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut key = [0u8; 32];
                // The OS CSPRNG. Any other source here would be the bug.
                getrandom::fill(&mut key).map_err(|e| SecretError::Io {
                    context: "generating a key".to_owned(),
                    source: std::io::Error::other(e),
                })?;
                write_private(&path, &key)?;
                let cipher = Self::new(&key);
                key.zeroize();
                Ok(cipher)
            }
            Err(source) => Err(SecretError::Io {
                context: format!("reading {}", path.display()),
                source,
            }),
        }
    }

    /// Encrypts `plaintext`, bound to `purpose`.
    pub fn seal(&self, purpose: &str, plaintext: &str) -> SecretResult<String> {
        use chacha20poly1305::aead::{Aead, Payload};

        // 192 bits from the OS CSPRNG. At that width a fresh random nonce per
        // value has negligible collision probability, which is what removes
        // the need for a counter -- the part of nonce handling that usually
        // goes wrong.
        let mut nonce_bytes = [0u8; 24];
        getrandom::fill(&mut nonce_bytes).map_err(|e| SecretError::Io {
            context: "generating a nonce".to_owned(),
            source: std::io::Error::other(e),
        })?;
        let nonce = XNonce::from(nonce_bytes);
        let ciphertext = self
            .inner
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: purpose.as_bytes(),
                },
            )
            .map_err(|_| SecretError::Undecryptable)?;

        Ok(format!(
            "{VERSION}:{}:{}",
            B64.encode(nonce),
            B64.encode(ciphertext)
        ))
    }

    /// Decrypts a value sealed under the same `purpose`.
    pub fn open(&self, purpose: &str, sealed: &str) -> SecretResult<String> {
        use chacha20poly1305::aead::{Aead, Payload};

        let mut parts = sealed.split(':');
        let (Some(version), Some(nonce), Some(ciphertext), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(SecretError::Malformed);
        };
        if version != VERSION {
            return Err(SecretError::Malformed);
        }

        let nonce = B64.decode(nonce).map_err(|_| SecretError::Malformed)?;
        let nonce: &XNonce = nonce
            .as_slice()
            .try_into()
            .map_err(|_| SecretError::Malformed)?;
        let ciphertext = B64.decode(ciphertext).map_err(|_| SecretError::Malformed)?;

        let plaintext = self
            .inner
            .decrypt(
                nonce,
                Payload {
                    msg: &ciphertext,
                    aad: purpose.as_bytes(),
                },
            )
            .map_err(|_| SecretError::Undecryptable)?;

        String::from_utf8(plaintext).map_err(|_| SecretError::Undecryptable)
    }
}

fn decode_key(encoded: &str) -> Option<[u8; 32]> {
    B64.decode(encoded).ok()?.as_slice().try_into().ok()
}

/// Writes key material readable only by its owner.
///
/// The mode is set at creation rather than afterwards, so the key is never
/// briefly world-readable.
fn write_private(path: &Path, bytes: &[u8]) -> SecretResult<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| SecretError::Io {
            context: format!("creating {}", path.display()),
            source,
        })?;

    file.write_all(bytes).map_err(|source| SecretError::Io {
        context: format!("writing {}", path.display()),
        source,
    })
}

/// Path of the generated key file.
#[must_use]
pub fn key_path(data_dir: &Path) -> PathBuf {
    data_dir.join(KEY_FILE)
}
