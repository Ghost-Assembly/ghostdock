//! Slug validation, which is a path-traversal boundary.

use compose::slug::{MAX_LEN, SlugError, from_name, validate};

#[test]
fn accepts_ordinary_names() {
    for good in [
        "blog",
        "media-server",
        "adguard_home",
        "x",
        "s3proxy",
        "app2",
    ] {
        assert!(validate(good).is_ok(), "{good} should be valid");
    }
}

#[test]
fn rejects_anything_that_could_escape_the_stacks_directory() {
    // The whole point: a slug becomes a directory name.
    for bad in [
        "..",
        "../etc",
        "a/../../etc",
        "a/b",
        "a\\b",
        "/absolute",
        "a\0b",
        "a b",
        "a:b",
        ".hidden",
    ] {
        assert!(
            validate(bad).is_err(),
            "{bad:?} must be rejected before it becomes a path"
        );
    }
}

#[test]
fn rejects_names_compose_itself_would_refuse() {
    assert_eq!(validate(""), Err(SlugError::Empty));
    assert_eq!(validate("-leading"), Err(SlugError::BadStart));
    assert_eq!(validate("_leading"), Err(SlugError::BadStart));
    assert_eq!(validate("UPPER"), Err(SlugError::BadStart));
    assert_eq!(validate("has UPPER"), Err(SlugError::BadCharacter));
    assert_eq!(validate(&"a".repeat(MAX_LEN + 1)), Err(SlugError::TooLong));
    assert!(
        validate(&"a".repeat(MAX_LEN)).is_ok(),
        "the boundary is allowed"
    );
}

#[test]
fn derives_a_slug_from_a_human_name() {
    assert_eq!(from_name("My Blog").as_deref(), Some("my-blog"));
    assert_eq!(
        from_name("  Media Server  ").as_deref(),
        Some("media-server")
    );
    assert_eq!(from_name("AdGuard Home").as_deref(), Some("adguard-home"));
    assert_eq!(
        from_name("docker-compose.yml").as_deref(),
        Some("docker-compose-yml")
    );
}

#[test]
fn derivation_never_produces_something_invalid() {
    for name in ["../../etc/passwd", "!!!", "   ", "", "-", "___", "🙂"] {
        if let Some(slug) = from_name(name) {
            assert!(
                validate(&slug).is_ok(),
                "derived {slug:?} from {name:?} but it is not valid"
            );
        }
    }
    // Nothing usable survives, so refuse rather than invent a name.
    assert_eq!(from_name("!!!"), None);
    assert_eq!(from_name(""), None);
    assert_eq!(from_name("🙂"), None);
}
