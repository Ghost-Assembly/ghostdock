//! Secrets at rest.
//!
//! These assert the properties that make encryption worth having, not merely
//! that bytes change: authentication, purpose binding, and nonce freshness.

use std::os::unix::fs::PermissionsExt;

use store::secrets::{Cipher, SecretError, key_path, purpose};

const KEY: [u8; 32] = [7; 32];
const OTHER_KEY: [u8; 32] = [9; 32];
const SECRET: &str = "ghp_a_private_repository_token";

#[test]
fn a_sealed_value_round_trips() {
    let cipher = Cipher::new(&KEY);
    let sealed = cipher.seal(purpose::GIT_CREDENTIAL, SECRET).unwrap();

    assert_ne!(sealed, SECRET);
    assert!(
        !sealed.contains(SECRET),
        "plaintext must not survive: {sealed}"
    );
    assert_eq!(
        cipher.open(purpose::GIT_CREDENTIAL, &sealed).unwrap(),
        SECRET
    );
}

#[test]
fn sealing_the_same_value_twice_gives_different_ciphertext() {
    // Otherwise the database leaks which stacks share a value.
    let cipher = Cipher::new(&KEY);
    let a = cipher.seal(purpose::STACK_ENV, SECRET).unwrap();
    let b = cipher.seal(purpose::STACK_ENV, SECRET).unwrap();

    assert_ne!(a, b, "each value needs its own nonce");
    assert_eq!(cipher.open(purpose::STACK_ENV, &a).unwrap(), SECRET);
    assert_eq!(cipher.open(purpose::STACK_ENV, &b).unwrap(), SECRET);
}

#[test]
fn a_value_cannot_be_opened_under_a_different_purpose() {
    // The database's structure is not itself authenticated, so without this
    // a sealed Git token could be moved into an environment variable column
    // and would decrypt there.
    let cipher = Cipher::new(&KEY);
    let sealed = cipher.seal(purpose::GIT_CREDENTIAL, SECRET).unwrap();

    assert!(matches!(
        cipher.open(purpose::STACK_ENV, &sealed),
        Err(SecretError::Undecryptable)
    ));
}

#[test]
fn a_value_cannot_be_opened_with_a_different_key() {
    let sealed = Cipher::new(&KEY).seal(purpose::STACK_ENV, SECRET).unwrap();

    assert!(matches!(
        Cipher::new(&OTHER_KEY).open(purpose::STACK_ENV, &sealed),
        Err(SecretError::Undecryptable)
    ));
}

#[test]
fn tampering_is_detected() {
    let cipher = Cipher::new(&KEY);
    let sealed = cipher.seal(purpose::STACK_ENV, SECRET).unwrap();

    // Flip a character in the ciphertext segment.
    let mut parts: Vec<&str> = sealed.split(':').collect();
    let last = parts.pop().unwrap().to_owned();
    let flipped = format!(
        "{}{}",
        if last.starts_with('A') { "B" } else { "A" },
        &last[1..]
    );
    let tampered = format!("{}:{flipped}", parts.join(":"));

    assert!(
        matches!(
            cipher.open(purpose::STACK_ENV, &tampered),
            Err(SecretError::Undecryptable)
        ),
        "an altered value must not decrypt"
    );
}

#[test]
fn nonsense_input_fails_cleanly_rather_than_panicking() {
    let cipher = Cipher::new(&KEY);
    for bad in [
        "",
        "not-sealed",
        "v1:only-two",
        "v9:aaaa:bbbb",
        "v1:!!!:!!!",
    ] {
        assert!(
            cipher.open(purpose::STACK_ENV, bad).is_err(),
            "{bad:?} should be an error, not a panic"
        );
    }
}

#[test]
fn an_empty_value_is_still_sealed() {
    let cipher = Cipher::new(&KEY);
    let sealed = cipher.seal(purpose::STACK_ENV, "").unwrap();
    assert_eq!(cipher.open(purpose::STACK_ENV, &sealed).unwrap(), "");
}

#[test]
fn the_generated_key_file_is_private_and_stable() {
    let dir = tempfile::tempdir().unwrap();

    let first = Cipher::load_or_create(dir.path(), None).unwrap();
    let sealed = first.seal(purpose::STACK_ENV, SECRET).unwrap();

    let mode = std::fs::metadata(key_path(dir.path()))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the key must not be world-readable");

    // A restart must be able to read back everything sealed before it.
    let second = Cipher::load_or_create(dir.path(), None).unwrap();
    assert_eq!(
        second.open(purpose::STACK_ENV, &sealed).unwrap(),
        SECRET,
        "regenerating the key would silently destroy every stored secret"
    );
}

#[test]
fn a_supplied_key_is_used_in_preference_to_the_file() {
    use base64::Engine as _;
    let dir = tempfile::tempdir().unwrap();
    let supplied = base64::engine::general_purpose::STANDARD.encode(KEY);

    let cipher = Cipher::load_or_create(dir.path(), Some(&supplied)).unwrap();
    let sealed = cipher.seal(purpose::STACK_ENV, SECRET).unwrap();

    assert!(
        !key_path(dir.path()).exists(),
        "a supplied key means no key file should be written at all"
    );
    assert_eq!(
        Cipher::new(&KEY).open(purpose::STACK_ENV, &sealed).unwrap(),
        SECRET
    );
}

#[test]
fn a_bad_key_in_the_environment_is_refused_rather_than_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let result = Cipher::load_or_create(dir.path(), Some("obviously-not-base64-32-bytes!!"));

    assert!(
        matches!(result, Err(SecretError::BadKeyFromEnv)),
        "silently falling back to a generated key would encrypt with a key \
         the operator did not choose"
    );
}
