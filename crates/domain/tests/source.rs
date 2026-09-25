//! What may be handed to git as a repository URL or a ref.

use domain::source::{SourceProblem, check_git_ref, check_repo_url};

#[test]
fn ordinary_urls_are_accepted() {
    for url in [
        "https://github.com/owner/repo.git",
        "http://git.example.internal/team/stacks",
        "ssh://git@example.invalid:2222/owner/repo.git",
        "git://example.invalid/repo.git",
        "file:///srv/git/stacks.git",
        "git@github.com:owner/repo.git",
        "example.invalid:stacks.git",
        "HTTPS://example.invalid/r.git",
        // A username alone is not a secret; hosts often want one.
        "https://oauth2@gitlab.example.invalid/o/r.git",
    ] {
        assert_eq!(check_repo_url(url), Ok(()), "{url}");
    }
}

#[test]
fn a_url_that_git_would_read_as_an_option_is_refused() {
    // git takes `--upload-pack=<command>` from wherever it finds it, and
    // runs the command.
    for url in ["--upload-pack=touch /tmp/pwned", "-u", "-"] {
        assert_eq!(
            check_repo_url(url),
            Err(SourceProblem::LeadingDash),
            "{url}"
        );
    }
}

#[test]
fn whitespace_and_control_characters_are_refused() {
    for url in [
        "https://example.invalid/r .git",
        "https://example.invalid/r.git\n",
        "https://example.invalid/\tr.git",
        "https://example.invalid/r\u{0}.git",
        "https://example.invalid/r\u{7f}.git",
    ] {
        assert_eq!(
            check_repo_url(url),
            Err(SourceProblem::Whitespace),
            "{url:?}"
        );
    }
}

#[test]
fn remote_helpers_and_other_transports_are_refused() {
    // `ext::` runs an arbitrary command; the others reach code GhostDock
    // never meant to offer.
    for url in [
        "ext::sh -c touch% /tmp/pwned",
        "ext::sh",
        "fd::7",
        "persistent-https::example.invalid/r.git",
        "ftp://example.invalid/r.git",
        "rsync://example.invalid/r.git",
        "git+ssh://example.invalid/r.git",
        "/srv/git/stacks.git",
        "relative/path",
        "",
    ] {
        let problem = check_repo_url(url).expect_err(url);
        assert!(
            matches!(
                problem,
                SourceProblem::Scheme | SourceProblem::Whitespace | SourceProblem::Empty
            ),
            "{url}: {problem:?}"
        );
    }
}

#[test]
fn a_password_in_the_url_is_refused_in_favor_of_a_credential() {
    for url in [
        "https://user:hunter2@example.invalid/r.git",
        "http://user:@example.invalid/r.git",
        "ssh://git:pw@example.invalid/r.git",
    ] {
        assert_eq!(
            check_repo_url(url),
            Err(SourceProblem::PasswordInUrl),
            "{url}"
        );
    }
    assert!(
        SourceProblem::PasswordInUrl
            .to_string()
            .contains("Credentials"),
        "the refusal says where the password belongs"
    );
}

#[test]
fn a_url_needs_a_host() {
    for url in ["https://", "https:///r.git", "ssh://git@/r.git", ":path"] {
        assert!(check_repo_url(url).is_err(), "{url}");
    }
}

#[test]
fn ordinary_refs_are_accepted() {
    for r in [
        "refs/heads/main",
        "main",
        "v1.2.3",
        "refs/tags/release-2026",
    ] {
        assert_eq!(check_git_ref(r), Ok(()), "{r}");
    }
}

#[test]
fn a_ref_that_git_would_read_as_an_option_is_refused() {
    assert_eq!(
        check_git_ref("--upload-pack=touch /tmp/pwned"),
        Err(SourceProblem::RefLeadingDash)
    );
    assert_eq!(check_git_ref("-b"), Err(SourceProblem::RefLeadingDash));
}

#[test]
fn a_ref_with_whitespace_or_control_characters_is_refused() {
    for r in ["refs/heads/ma in", "main\n", "\tmain", "ma\u{1b}in"] {
        assert_eq!(check_git_ref(r), Err(SourceProblem::RefWhitespace), "{r:?}");
    }
    assert_eq!(check_git_ref(""), Err(SourceProblem::RefEmpty));
}
