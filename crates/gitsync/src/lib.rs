//! Git remote polling and working-tree materialisation.
//!
//! Change detection uses `git ls-remote` against the tracked ref, which
//! costs one round-trip and no clone. Like `compose`, this shells out to
//! the reference implementation so credential helpers, submodules, and
//! LFS behave exactly as they do on the command line.

pub mod command;

use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use command::Credential;
use tokio::process::Command;

/// How long any single git invocation may take.
///
/// Bounded so an unreachable host cannot hold a stack's slot indefinitely,
/// but generous enough for a large first fetch.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not run git. Is it installed and on PATH? ({0})")]
    Spawn(#[source] std::io::Error),
    #[error("git timed out after {0} seconds")]
    Timeout(u64),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    /// Carries git's own message, which is usually the useful part.
    #[error("{0}")]
    Git(String),
    #[error("path {0:?} must stay inside the repository")]
    PathEscapes(String),
    #[error("{0} is not in the repository")]
    NotInRepo(String),
    /// Authentication failed, explained, with git's own words kept after.
    #[error("{} (git said: {detail})", .problem.explain())]
    Auth {
        problem: AuthProblem,
        detail: String,
    },
}

/// Why a remote turned GhostDock away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthProblem {
    /// A credential was sent and refused.
    Refused,
    /// None was sent and the remote wants one.
    Needed,
    /// The credential was accepted but may not read this repository.
    NoAccess,
    /// The remote says the repository does not exist, which is also how
    /// GitHub answers when the credential cannot see a private repository.
    NotVisible,
}

impl AuthProblem {
    #[must_use]
    pub fn explain(self) -> &'static str {
        match self {
            Self::Refused => {
                "The repository refused the credential. Check that the token is still valid \
                 (tokens expire) and that it can read this repository."
            }
            Self::Needed => {
                "The repository needs a credential. Add one under Sources and attach it \
                 to this repository."
            }
            Self::NoAccess => {
                "The credential is valid but cannot read this repository. For a GitHub \
                 fine-grained token: set the resource owner to the account or organisation \
                 that owns the repository, include the repository, and grant Contents: \
                 read-only. An organisation may also need to approve the token."
            }
            Self::NotVisible => {
                "The repository was not found. Check the URL, and that the credential has \
                 access to it; a private repository looks missing to a token that cannot see it."
            }
        }
    }
}

/// Recognises an authentication failure in git's stderr.
///
/// Needed because git's wording depends on its version: older releases
/// report a rejected credential by trying to prompt for a username, which
/// reads exactly like never having been given one. Knowing whether a
/// credential was sent is what tells the two apart.
#[must_use]
pub fn auth_problem(stderr: &str, credential_sent: bool) -> Option<AuthProblem> {
    let lower = stderr.to_lowercase();
    if lower.contains("repository not found") || lower.contains("' not found") {
        return Some(AuthProblem::NotVisible);
    }
    if lower.contains("access to repository not granted") || lower.contains("returned error: 403") {
        return Some(AuthProblem::NoAccess);
    }
    let auth = [
        "could not read username",
        "could not read password",
        "terminal prompts disabled",
        "authentication failed",
        "invalid username or token",
        "access denied",
    ];
    auth.iter()
        .any(|p| lower.contains(p))
        .then_some(if credential_sent {
            AuthProblem::Refused
        } else {
            AuthProblem::Needed
        })
}

pub type Result<T> = std::result::Result<T, Error>;

/// Resolves a repository-relative path, refusing anything that escapes.
///
/// The compose file path is configuration, so it is attacker-adjacent
/// whenever someone can register a stack. A whitelist of ordinary components
/// is used rather than a search for `..`, since a blacklist only ever covers
/// the encodings someone has thought of.
pub fn resolve_in_repo(root: &Path, relative: &str) -> Result<PathBuf> {
    let candidate = Path::new(relative);
    if candidate.is_absolute() {
        return Err(Error::PathEscapes(relative.to_owned()));
    }

    let mut resolved = root.to_path_buf();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => resolved.push(part),
            // A leading `./` is harmless but adds nothing.
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::PathEscapes(relative.to_owned()));
            }
        }
    }

    if resolved == root {
        return Err(Error::PathEscapes(relative.to_owned()));
    }
    Ok(resolved)
}

/// Runs git against remotes and working trees.
#[derive(Debug, Clone)]
pub struct Git {
    bin: String,
    timeout: Duration,
}

impl Default for Git {
    fn default() -> Self {
        Self::new()
    }
}

impl Git {
    #[must_use]
    pub fn new() -> Self {
        Self {
            bin: std::env::var("GHOSTDOCK_GIT_BIN").unwrap_or_else(|_| "git".to_owned()),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// The commit a remote ref currently points at.
    ///
    /// Cheap enough to poll for every stack on a timer, which is the whole
    /// reason change detection does not clone.
    pub async fn remote_head(
        &self,
        url: &str,
        reference: &str,
        credential: Option<&Credential>,
    ) -> Result<String> {
        let output = self
            .run(&command::ls_remote(url, reference), credential)
            .await?;

        // "<sha>\t<ref>", possibly several lines if the ref is ambiguous.
        output
            .split_whitespace()
            .next()
            .filter(|sha| sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()))
            .map(str::to_owned)
            .ok_or_else(|| Error::Git(format!("no ref matching {reference} on the remote")))
    }

    /// Brings `dir` to the current state of `reference`, returning its commit.
    ///
    /// The working tree is treated as a cache of one commit, never as
    /// somewhere edits live: it is forced to the fetched revision and cleaned
    /// of anything left by a previous one.
    pub async fn sync(
        &self,
        dir: &Path,
        url: &str,
        reference: &str,
        credential: Option<&Credential>,
    ) -> Result<String> {
        if !dir.join(".git").exists() {
            tokio::fs::create_dir_all(dir)
                .await
                .map_err(|source| Error::Io {
                    context: format!("creating {}", dir.display()),
                    source,
                })?;
            self.run(&command::init(dir), None).await?;
        }

        self.run(&command::fetch(dir, url, reference), credential)
            .await?;
        self.run(&command::checkout(dir), None).await?;
        self.run(&command::clean(dir), None).await?;

        Ok(self.run(&command::head(dir), None).await?.trim().to_owned())
    }

    /// Paths of every file the checkout tracks, relative to its root.
    pub async fn list_files(&self, dir: &Path) -> Result<Vec<String>> {
        let out = self.run(&command::ls_files(dir), None).await?;
        Ok(out
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect())
    }

    async fn run(&self, argv: &[String], credential: Option<&Credential>) -> Result<String> {
        let mut cmd = Command::new(&self.bin);
        cmd.args(argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        for (key, value) in command::env(credential) {
            cmd.env(key, value);
        }

        let output = tokio::time::timeout(self.timeout, cmd.output())
            .await
            .map_err(|_| Error::Timeout(self.timeout.as_secs()))?
            .map_err(Error::Spawn)?;

        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }

        // git explains itself on stderr, and that explanation is what a user
        // needs; a generic failure here would hide the reason entirely.
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if let Some(problem) = auth_problem(&message, credential.is_some()) {
            return Err(Error::Auth {
                problem,
                detail: message,
            });
        }
        Err(Error::Git(if message.is_empty() {
            format!("git exited with {}", output.status)
        } else {
            message
        }))
    }
}
