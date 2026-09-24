//! Building `git` invocations.
//!
//! Pure and separate, like the compose driver: argv and environment are
//! exactly where a credential leaks or the wrong ref gets checked out.

use std::path::Path;

/// A credential for reaching a remote over HTTPS.
///
/// For a token, the username is whatever the host expects alongside it
/// (GitHub accepts anything, GitLab wants `oauth2`).
#[derive(Clone)]
pub struct Credential {
    pub username: String,
    pub secret: String,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A credential must never reach a log through a derived Debug.
        f.debug_struct("Credential")
            .field("username", &self.username)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// Environment for a git invocation.
///
/// Credentials travel in the environment rather than in argv. On Linux a
/// process's arguments are world-readable through `/proc/<pid>/cmdline`,
/// while its environment is readable only by its owner, so a token in argv
/// is visible to every user on the host for as long as the command runs.
/// Embedding it in the URL would be worse still: git writes remote URLs into
/// `.git/config` and into its own error messages.
#[must_use]
pub fn env(credential: Option<&Credential>) -> Vec<(String, String)> {
    let mut vars = vec![
        // Never block waiting for a username on a machine with no terminal.
        ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
        // Read neither the system's nor the user's configuration. A
        // ~/.gitconfig often holds a credential helper (gh, a keychain), and
        // honouring it would lend GhostDock its host user's access unasked:
        // a private repository would open with no credential configured.
        // GhostDock supplies exactly what it means to and nothing else.
        ("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()),
        ("GIT_CONFIG_GLOBAL".to_owned(), "/dev/null".to_owned()),
        ("GIT_ASKPASS".to_owned(), String::new()),
    ];

    if let Some(credential) = credential {
        use base64::Engine as _;
        let basic = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", credential.username, credential.secret));

        // `GIT_CONFIG_*` applies config without a file and without argv.
        vars.extend([
            ("GIT_CONFIG_COUNT".to_owned(), "1".to_owned()),
            ("GIT_CONFIG_KEY_0".to_owned(), "http.extraHeader".to_owned()),
            (
                "GIT_CONFIG_VALUE_0".to_owned(),
                format!("Authorization: Basic {basic}"),
            ),
        ]);
    }

    vars
}

/// Asks the remote what a ref currently points at, without cloning.
///
/// One round trip and no working tree, which is what makes polling cheap
/// enough to do on a timer for every stack.
#[must_use]
pub fn ls_remote(url: &str, reference: &str) -> Vec<String> {
    vec![
        "ls-remote".to_owned(),
        "--exit-code".to_owned(),
        url.to_owned(),
        reference.to_owned(),
    ]
}

/// Prepares an empty repository in `dir`.
#[must_use]
pub fn init(dir: &Path) -> Vec<String> {
    vec![
        "init".to_owned(),
        "--quiet".to_owned(),
        dir.display().to_string(),
    ]
}

/// Fetches one ref, shallowly.
///
/// History is not wanted: GhostDock only ever needs the tree at one commit, and
/// a deep clone of a monorepo is a slow way to read one file.
#[must_use]
pub fn fetch(dir: &Path, url: &str, reference: &str) -> Vec<String> {
    vec![
        "-C".to_owned(),
        dir.display().to_string(),
        "fetch".to_owned(),
        "--quiet".to_owned(),
        "--depth".to_owned(),
        "1".to_owned(),
        // A force fetch, so a rewritten branch does not wedge the mirror.
        "--force".to_owned(),
        url.to_owned(),
        reference.to_owned(),
    ]
}

/// Moves the working tree to what was just fetched, discarding local state.
#[must_use]
pub fn checkout(dir: &Path) -> Vec<String> {
    vec![
        "-C".to_owned(),
        dir.display().to_string(),
        "checkout".to_owned(),
        "--quiet".to_owned(),
        "--force".to_owned(),
        "FETCH_HEAD".to_owned(),
    ]
}

/// The commit currently checked out.
#[must_use]
pub fn head(dir: &Path) -> Vec<String> {
    vec![
        "-C".to_owned(),
        dir.display().to_string(),
        "rev-parse".to_owned(),
        "HEAD".to_owned(),
    ]
}

/// Removes files left by a previous revision that are no longer tracked.
#[must_use]
pub fn clean(dir: &Path) -> Vec<String> {
    vec![
        "-C".to_owned(),
        dir.display().to_string(),
        "clean".to_owned(),
        "--quiet".to_owned(),
        "-fd".to_owned(),
    ]
}

/// Every tracked file, NUL-separated so no file name can be misread.
#[must_use]
pub fn ls_files(dir: &Path) -> Vec<String> {
    vec![
        "-C".to_owned(),
        dir.display().to_string(),
        "ls-files".to_owned(),
        "-z".to_owned(),
    ]
}
