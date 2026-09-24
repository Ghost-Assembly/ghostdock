//! Which WebSocket handshakes are accepted.
//!
//! The exec socket is a root shell in a container, and WebSocket handshakes
//! are not covered by the same-origin policy, so this is the check standing
//! between a malicious page and that shell.

use server::origin::{is_allowed, parse_allowlist};

const HOST: &str = "ghostdock.example:8080";

#[test]
fn a_matching_origin_is_accepted() {
    assert!(is_allowed(
        Some("http://ghostdock.example:8080"),
        Some(HOST),
        &[]
    ));
    assert!(is_allowed(
        Some("https://ghostdock.example:8080"),
        Some(HOST),
        &[]
    ));
}

#[test]
fn another_site_is_refused() {
    // The attack: a page somewhere else opens a socket, and the browser
    // attaches the session cookie to it.
    for evil in [
        "http://evil.example",
        "https://evil.example",
        "http://ghostdock.example.evil.example",
        "http://ghostdock.example:9999",
        "null",
    ] {
        assert!(
            !is_allowed(Some(evil), Some(HOST), &[]),
            "{evil} must not be able to open a shell"
        );
    }
}

#[test]
fn a_prefix_of_the_host_is_not_the_host() {
    assert!(!is_allowed(
        Some("http://ghostdock.exampl"),
        Some(HOST),
        &[]
    ));
    assert!(!is_allowed(
        Some("http://ghostdock.example"),
        Some(HOST),
        &[]
    ));
}

#[test]
fn an_absent_origin_is_allowed() {
    // Browsers always send one on a WebSocket handshake, so its absence
    // means a non-browser client, which is not what this guards against.
    assert!(is_allowed(None, Some(HOST), &[]));
}

#[test]
fn an_allowlisted_origin_is_accepted_whatever_the_host() {
    // For a reverse proxy that does not preserve the original Host.
    let allowed = parse_allowlist(Some("https://ghostdock.public.example"));
    assert!(is_allowed(
        Some("https://ghostdock.public.example"),
        Some("127.0.0.1:8080"),
        &allowed
    ));
    assert!(!is_allowed(
        Some("https://other.example"),
        Some("127.0.0.1:8080"),
        &allowed
    ));
}

#[test]
fn an_origin_with_no_host_to_compare_against_is_refused() {
    assert!(!is_allowed(
        Some("http://ghostdock.example:8080"),
        None,
        &[]
    ));
}

#[test]
fn a_malformed_origin_is_refused_rather_than_guessed_at() {
    for bad in [
        "ghostdock.example:8080",
        "ftp://ghostdock.example:8080",
        "http://",
        "http://ghostdock.example:8080/path",
    ] {
        assert!(
            !is_allowed(Some(bad), Some(HOST), &[]),
            "{bad} must be refused"
        );
    }
}

#[test]
fn an_allowlist_is_read_forgivingly() {
    let allowed = parse_allowlist(Some(" https://a.example , https://b.example ,, "));
    assert_eq!(allowed, ["https://a.example", "https://b.example"]);
    assert!(parse_allowlist(None).is_empty());
    assert!(parse_allowlist(Some("")).is_empty());
}
