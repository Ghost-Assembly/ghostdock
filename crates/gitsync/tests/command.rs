//! Git invocations, with attention to where a credential could leak.

use std::path::Path;

use gitsync::command::{self, Credential};

const TOKEN: &str = "ghp_supersecrettoken";

fn credential() -> Credential {
    Credential {
        username: "x-access-token".to_owned(),
        secret: TOKEN.to_owned(),
    }
}

fn dir() -> &'static Path {
    Path::new("/var/lib/ghostdock/stacks/blog/repo")
}

#[test]
fn no_argv_ever_contains_the_secret() {
    // On Linux /proc/<pid>/cmdline is world-readable, so a token in argv is
    // visible to every user on the host while the command runs.
    let invocations = [
        command::ls_remote("https://example.invalid/r.git", "refs/heads/main"),
        command::init(dir()),
        command::fetch(dir(), "https://example.invalid/r.git", "refs/heads/main"),
        command::checkout(dir()),
        command::head(dir()),
        command::clean(dir()),
    ];

    for argv in invocations {
        let joined = argv.join(" ");
        assert!(!joined.contains(TOKEN), "secret leaked into argv: {joined}");
        assert!(
            !joined.contains("@example.invalid"),
            "a credential must not be embedded in the URL: {joined}"
        );
    }
}

#[test]
fn a_credential_travels_in_the_environment_as_a_header() {
    let vars = command::env(Some(&credential()));
    let map: std::collections::HashMap<_, _> = vars.into_iter().collect();

    assert_eq!(map.get("GIT_CONFIG_COUNT").map(String::as_str), Some("1"));
    assert_eq!(
        map.get("GIT_CONFIG_KEY_0").map(String::as_str),
        Some("http.extraHeader")
    );
    let header = map.get("GIT_CONFIG_VALUE_0").expect("header");
    assert!(header.starts_with("Authorization: Basic "));

    // Base64 of "x-access-token:ghp_supersecrettoken".
    use base64::Engine as _;
    let encoded = header.trim_start_matches("Authorization: Basic ");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    assert_eq!(
        String::from_utf8(decoded).unwrap(),
        format!("x-access-token:{TOKEN}")
    );
}

#[test]
fn git_never_waits_for_a_human() {
    // A server has no terminal; without this git blocks forever on a private
    // repository instead of failing.
    let map: std::collections::HashMap<_, _> = command::env(None).into_iter().collect();
    assert_eq!(
        map.get("GIT_TERMINAL_PROMPT").map(String::as_str),
        Some("0")
    );
    assert_eq!(map.get("GIT_ASKPASS").map(String::as_str), Some(""));
}

#[test]
fn without_a_credential_no_authorization_is_configured() {
    let map: std::collections::HashMap<_, _> = command::env(None).into_iter().collect();
    assert!(!map.contains_key("GIT_CONFIG_COUNT"));
    assert!(!map.contains_key("GIT_CONFIG_VALUE_0"));
}

#[test]
fn a_credential_is_not_printed_by_debug() {
    let rendered = format!("{:?}", credential());
    assert!(
        !rendered.contains(TOKEN),
        "Debug leaked the secret: {rendered}"
    );
    assert!(rendered.contains("redacted"));
}

#[test]
fn fetching_is_shallow_and_forced() {
    let argv = command::fetch(dir(), "https://example.invalid/r.git", "refs/heads/main");
    let position = |flag: &str| argv.iter().position(|a| a == flag);

    assert!(position("--depth").is_some(), "history is never needed");
    assert_eq!(argv[position("--depth").unwrap() + 1], "1");
    assert!(
        position("--force").is_some(),
        "a rewritten branch must not wedge the mirror"
    );
}

#[test]
fn checkout_and_clean_discard_local_state() {
    // The working tree is a cache of a commit, never somewhere to keep edits.
    assert!(command::checkout(dir()).contains(&"--force".to_owned()));
    assert!(command::clean(dir()).contains(&"-fd".to_owned()));
}

#[test]
fn ls_remote_fails_when_a_ref_is_missing() {
    // Without --exit-code, asking for a branch that does not exist succeeds
    // with empty output, which reads as "no change" rather than an error.
    assert!(
        command::ls_remote("https://example.invalid/r.git", "refs/heads/gone")
            .contains(&"--exit-code".to_owned())
    );
}

// ---- explaining authentication failures ----------------------------------

use gitsync::{AuthProblem, auth_problem};

#[test]
fn a_refused_credential_is_recognised_across_git_versions() {
    // Older git reports a rejected header by trying to prompt, which reads
    // exactly like having sent nothing; newer git says what happened.
    for stderr in [
        "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
        "remote: Invalid username or token. Password authentication is not supported for Git operations.\nfatal: Authentication failed for 'https://github.com/o/r.git/'",
        "remote: HTTP Basic: Access denied\nfatal: Authentication failed for 'https://gitlab.com/o/r.git/'",
    ] {
        assert_eq!(
            auth_problem(stderr, true),
            Some(AuthProblem::Refused),
            "{stderr}"
        );
        assert_eq!(
            auth_problem(stderr, false),
            Some(AuthProblem::Needed),
            "{stderr}"
        );
    }
}

#[test]
fn a_repository_hidden_from_the_credential_is_recognised() {
    // GitHub answers "not found" rather than "forbidden" for a private
    // repository the token cannot see.
    let stderr =
        "remote: Repository not found.\nfatal: repository 'https://github.com/o/r.git/' not found";
    assert_eq!(auth_problem(stderr, true), Some(AuthProblem::NotVisible));
    assert_eq!(auth_problem(stderr, false), Some(AuthProblem::NotVisible));
}

#[test]
fn a_credential_without_access_is_recognised() {
    // GitHub's words for a fine-grained token that is valid but was not
    // given this repository, even when only reading.
    let stderr = "remote: Write access to repository not granted.\nfatal: unable to access 'https://github.com/o/r.git/': The requested URL returned error: 403";
    assert_eq!(auth_problem(stderr, true), Some(AuthProblem::NoAccess));
    assert!(AuthProblem::NoAccess.explain().contains("Contents"));
}

#[test]
fn other_failures_are_left_as_git_said_them() {
    for stderr in [
        "fatal: couldn't find remote ref refs/heads/nope",
        "fatal: unable to access 'https://x/': Could not resolve host: x",
    ] {
        assert_eq!(auth_problem(stderr, true), None, "{stderr}");
    }
}

#[test]
fn the_explanation_says_what_to_do() {
    assert!(AuthProblem::Refused.explain().contains("still valid"));
    assert!(AuthProblem::Needed.explain().contains("credential"));
    assert!(AuthProblem::NotVisible.explain().contains("access"));
}

#[test]
fn git_never_reads_the_hosts_own_configuration() {
    // A developer's ~/.gitconfig commonly holds a credential helper (gh,
    // a keychain). Read, it would lend GhostDock that person's access without
    // anyone choosing to, and make a repository look public when it is not.
    let env = gitsync::command::env(None);
    let get = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    assert_eq!(get("GIT_CONFIG_NOSYSTEM"), Some("1"));
    assert_eq!(get("GIT_CONFIG_GLOBAL"), Some("/dev/null"));
}
