//! Password hashing.
//!
//! Argon2id via the RustCrypto implementation. This is the one area where
//! writing our own would be straightforwardly wrong, so the crate is used
//! as-is; what lives here is only the policy around it.

/// Minimum password length accepted at registration.
///
/// Length dominates composition rules for resistance to guessing, so this is
/// the only constraint imposed — no character-class theatre.
pub const MIN_PASSWORD_LEN: usize = 12;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PasswordError {
    #[error("password must be at least {MIN_PASSWORD_LEN} characters")]
    TooShort,
    #[error("password hashing failed")]
    Hash,
    /// The stored hash could not be parsed. Treated as a verification
    /// failure by callers, never as a reason to admit the user.
    #[error("stored password hash is malformed")]
    MalformedHash,
}

/// Hashes `password` into a PHC string suitable for storage.
pub fn hash_password(password: &str) -> Result<String, PasswordError> {
    use argon2::Argon2;
    use argon2::password_hash::PasswordHasher;

    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(PasswordError::TooShort);
    }

    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|_| PasswordError::Hash)
}

/// Checks `password` against a stored PHC string.
///
/// Returns `false` for a wrong password *and* for an unparseable hash;
/// a corrupt row must never authenticate anybody.
#[must_use]
pub fn verify_password(password: &str, stored: &str) -> bool {
    use argon2::Argon2;
    use argon2::password_hash::{PasswordVerifier, phc::PasswordHash};

    // Parse failures are deliberately swallowed into `false`. Surfacing them
    // would let a corrupt row become an authentication bypass if any caller
    // ever treated "could not check" as "no objection".
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}
