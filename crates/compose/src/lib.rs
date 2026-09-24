//! Drives the official `docker compose` CLI.
//!
//! ARCHITECTURAL INVARIANT: this is the ONLY writer to the container
//! world, and it must not know about HTTP or the database.
//!
//! We shell out rather than reimplement Compose so that spec fidelity is
//! guaranteed by construction. `docker compose config --format json` is
//! our parser: the UI and the deployer therefore can never disagree about
//! what a compose file means.

pub mod command;
pub mod env;
pub mod slug;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

pub const COMPOSE_FILE: &str = "docker-compose.yml";
pub const ENV_FILE: &str = ".env";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid stack name: {0}")]
    Slug(#[from] slug::SlugError),
    #[error("invalid environment: {0}")]
    Env(#[from] env::EnvError),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not run `{bin} compose`. Is the Docker CLI installed and on PATH? ({source})")]
    Spawn {
        bin: String,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// The result of one `docker compose` invocation.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// True only when compose exited zero. With `up --wait` that means the
    /// services really are running or healthy, not merely that they started.
    pub success: bool,
    pub exit_code: Option<i32>,
    /// Whether the command was killed for exceeding its timeout.
    pub timed_out: bool,
    /// Merged stdout and stderr, in the order it was produced.
    pub output: String,
    pub duration: Duration,
}

impl Outcome {
    /// The last few lines, for a summary where the whole log is too much.
    #[must_use]
    pub fn tail(&self, lines: usize) -> String {
        let all: Vec<&str> = self.output.lines().collect();
        all[all.len().saturating_sub(lines)..].join("\n")
    }
}

/// Runs `docker compose` against materialised project directories.
#[derive(Debug, Clone)]
pub struct Compose {
    bin: String,
    root: PathBuf,
}

impl Compose {
    /// `root` is the directory holding one subdirectory per stack.
    ///
    /// It must be reachable at the same absolute path as on the Docker host:
    /// the CLI resolves relative paths in a compose file client-side, while
    /// the daemon interprets the result, so the two must agree.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            bin: std::env::var("GHOSTDOCK_DOCKER_BIN").unwrap_or_else(|_| "docker".to_owned()),
            root: root.into(),
        }
    }

    #[must_use]
    pub fn project_dir(&self, stack: &str) -> PathBuf {
        self.root.join(stack)
    }

    /// Writes the compose file and `.env` for a stack.
    ///
    /// The slug is validated first: it becomes a directory name, so an
    /// unchecked one is a path-traversal primitive.
    pub async fn materialise(
        &self,
        stack: &str,
        compose_yaml: &str,
        vars: &[(String, String)],
    ) -> Result<PathBuf> {
        slug::validate(stack)?;
        let dir = self.project_dir(stack);

        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|source| Error::Io {
                context: format!("creating {}", dir.display()),
                source,
            })?;

        let compose_path = dir.join(COMPOSE_FILE);
        tokio::fs::write(&compose_path, compose_yaml)
            .await
            .map_err(|source| Error::Io {
                context: format!("writing {}", compose_path.display()),
                source,
            })?;

        let env_path = dir.join(ENV_FILE);
        if vars.is_empty() {
            // Remove a stale file, or compose keeps applying variables that
            // were deleted from the stack.
            let _ = tokio::fs::remove_file(&env_path).await;
        } else {
            let rendered = env::render(vars)?;
            write_private(&env_path, &rendered).await?;
        }

        Ok(dir)
    }

    /// Writes just the env file for a stack, returning its path.
    ///
    /// Used for a Git-backed stack, whose compose file is read from the
    /// checked-out repository and must not be copied or written beside.
    /// Returns `None` when the stack has no variables.
    pub async fn write_env_file(
        &self,
        stack: &str,
        vars: &[(String, String)],
    ) -> Result<Option<PathBuf>> {
        slug::validate(stack)?;
        let dir = self.project_dir(stack);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|source| Error::Io {
                context: format!("creating {}", dir.display()),
                source,
            })?;

        let path = dir.join(ENV_FILE);
        if vars.is_empty() {
            let _ = tokio::fs::remove_file(&path).await;
            return Ok(None);
        }

        write_private(&path, &env::render(vars)?).await?;
        Ok(Some(path))
    }

    /// Runs one invocation, streaming each output line to `sink` as it
    /// appears and returning the whole thing at the end.
    pub async fn run(
        &self,
        argv: &[String],
        timeout: Duration,
        sink: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<Outcome> {
        let started = Instant::now();

        let mut child = Command::new(&self.bin)
            .args(argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Compose prints progress with ANSI control codes when it thinks a
            // terminal is attached; plain output is what we want to store.
            .env("NO_COLOR", "1")
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| Error::Spawn {
                bin: self.bin.clone(),
                source,
            })?;

        let (tx, mut rx) = mpsc::unbounded_channel();
        pump(child.stdout.take(), tx.clone());
        // Compose writes its progress and its errors to stderr, so both
        // streams matter and are merged in the order they arrive.
        pump(child.stderr.take(), tx);

        let mut output = String::new();
        let mut timed_out = false;

        let status = tokio::select! {
            status = child.wait() => status.ok(),
            () = tokio::time::sleep(timeout) => {
                timed_out = true;
                let _ = child.start_kill();
                child.wait().await.ok()
            }
        };

        // Drain whatever the readers captured; the senders are dropped once
        // the pipes close, so this terminates.
        while let Some(line) = rx.recv().await {
            if let Some(sink) = &sink {
                let _ = sink.send(line.clone());
            }
            output.push_str(&line);
            output.push('\n');
        }

        Ok(Outcome {
            success: !timed_out && status.is_some_and(|s| s.success()),
            exit_code: status.and_then(|s| s.code()),
            timed_out,
            output,
            duration: started.elapsed(),
        })
    }
}

fn pump<R>(stream: Option<R>, tx: mpsc::UnboundedSender<String>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let Some(stream) = stream else { return };
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
}

/// Writes a file only the owner can read.
///
/// The `.env` holds a stack's secrets, so it must never be world-readable —
/// and the mode has to be set at creation, not after, or there is a window
/// where it is not.
async fn write_private(path: &Path, contents: &str) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .await
        .map_err(|source| Error::Io {
            context: format!("writing {}", path.display()),
            source,
        })?;

    file.write_all(contents.as_bytes())
        .await
        .map_err(|source| Error::Io {
            context: format!("writing {}", path.display()),
            source,
        })
}
