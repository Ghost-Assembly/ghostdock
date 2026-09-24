//! Password hashing policy.

use domain::auth::{MIN_PASSWORD_LEN, PasswordError, hash_password, verify_password};

const GOOD: &str = "correct horse battery staple";

#[test]
fn hash_then_verify_accepts_the_right_password() {
    let stored = hash_password(GOOD).unwrap();
    assert!(verify_password(GOOD, &stored));
}

#[test]
fn verify_rejects_the_wrong_password() {
    let stored = hash_password(GOOD).unwrap();
    assert!(!verify_password("not the password", &stored));
    assert!(!verify_password("", &stored));
}

#[test]
fn hashes_are_salted_so_equal_passwords_differ() {
    let a = hash_password(GOOD).unwrap();
    let b = hash_password(GOOD).unwrap();
    assert_ne!(
        a, b,
        "identical passwords must not produce identical hashes"
    );
    assert!(verify_password(GOOD, &a) && verify_password(GOOD, &b));
}

#[test]
fn hash_is_a_phc_argon2id_string() {
    let stored = hash_password(GOOD).unwrap();
    assert!(
        stored.starts_with("$argon2id$"),
        "expected argon2id PHC string, got: {stored}"
    );
}

#[test]
fn short_passwords_are_rejected() {
    let short = "a".repeat(MIN_PASSWORD_LEN - 1);
    assert_eq!(hash_password(&short), Err(PasswordError::TooShort));

    let exact = "a".repeat(MIN_PASSWORD_LEN);
    assert!(hash_password(&exact).is_ok(), "the boundary is accepted");
}

#[test]
fn a_corrupt_stored_hash_never_authenticates() {
    for bad in ["", "not-a-hash", "$argon2id$garbage", "$unknown$v=19$x"] {
        assert!(
            !verify_password(GOOD, bad),
            "malformed hash {bad:?} must not authenticate"
        );
    }
}
