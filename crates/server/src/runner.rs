//! Runs stack operations, one at a time per stack.
//!
//! Operations take minutes, so the HTTP request records the attempt and
//! returns; progress arrives over SSE. Everything that mutates the container
//! world goes through here.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use std::path::PathBuf;

use compose::command::{self, Project, Pull};
use compose::{COMPOSE_FILE, Compose};
use gitsync::Git;
use gitsync::command::Credential;
use shared::deployment::{Action, Deployment, RegisteredStack, Trigger};
use shared::event::ServerEvent;
use store::Store;
use tokio::sync::broadcast;

/// How long any single compose invocation may take before it is killed.
///
/// Generous, because a first deploy may be pulling several large images, but
/// bounded so a wedged command cannot hold a stack's slot forever.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How long `up --wait` waits for services to become healthy.
///
/// Shorter than the command timeout: this one is about the stack being slow
/// to come up, which is a result worth reporting rather than waiting out.
const WAIT_TIMEOUT_SECS: u64 = 180;

/// Capacity of the event channel.
///
/// A slow client that falls this far behind is disconnected rather than
/// allowed to hold output in memory for everyone else.
const EVENT_CAPACITY: usize = 512;

/// Where a stack's compose file and environment ended up.
struct Prepared {
    project_dir: PathBuf,
    compose_file: String,
    env_file: Option<PathBuf>,
    /// The commit the file came from, for a Git-backed stack.
    commit: Option<String>,
}

impl Prepared {
    fn project<'a>(&'a self, name: &'a str) -> Project<'a> {
        Project {
            name,
            dir: &self.project_dir,
            file: &self.compose_file,
            env_file: self.env_file.as_deref(),
        }
    }
}

/// What stop, restart and take-down act on.
enum Existing {
    /// The files the stack was last deployed from.
    Files(Prepared),
    /// No files: the project by name, found through its containers' labels.
    Name(PathBuf),
}

/// Every action except deploying, which is the only one that changes what
/// a stack is.
#[derive(Clone, Copy)]
enum Operation {
    Stop,
    Restart,
    TakeDown,
}

/// A stack's claim on running something. Released when dropped, so a task
/// that panics cannot leave its stack marked busy for good.
#[derive(Debug)]
pub struct Slot {
    in_flight: Arc<Mutex<HashSet<i64>>>,
    stack_id: i64,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.stack_id);
    }
}

/// The result of one attempt.
struct Outcome {
    success: bool,
    exit_code: Option<i32>,
    log: String,
    commit: Option<String>,
}

impl Outcome {
    fn failed(log: String) -> Self {
        Self {
            success: false,
            exit_code: None,
            log,
            commit: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("another operation is already running for this stack")]
    Busy,
    #[error(transparent)]
    Compose(#[from] compose::Error),
    #[error(transparent)]
    Store(#[from] store::Error),
    #[error(transparent)]
    Git(#[from] gitsync::Error),
    #[error(
        "GhostDock has no files for this stack, and compose would read {} instead. \
         Deploy it first.",
        .0.display()
    )]
    NoFiles(PathBuf),
}

#[derive(Clone)]
pub struct Runner {
    compose: Compose,
    git: Git,
    store: Store,
    events: broadcast::Sender<ServerEvent>,
    /// Stacks with an operation in flight.
    ///
    /// Concurrent compose invocations against one project race over the same
    /// containers, so a second request is refused rather than queued: the user
    /// is told what is happening instead of waiting silently.
    in_flight: Arc<Mutex<HashSet<i64>>>,
    discovering: Arc<tokio::sync::Mutex<()>>,
}

/// Where discovery checks repositories out, beside the stack projects. A
/// leading dot keeps it clear of every project name, which cannot start
/// with one.
const DISCOVERY_DIR: &str = ".discovery";

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner").finish_non_exhaustive()
    }
}

impl Runner {
    pub fn new(compose: Compose, store: Store) -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Self {
            compose,
            git: Git::new(),
            store,
            events,
            in_flight: Arc::new(Mutex::new(HashSet::new())),
            discovering: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<ServerEvent> {
        self.events.subscribe()
    }

    /// Sends an event from elsewhere down the same stream, so clients keep
    /// one connection for everything.
    pub fn publish(&self, event: ServerEvent) {
        // No subscribers just means no one is looking.
        let _ = self.events.send(event);
    }

    #[must_use]
    pub fn is_busy(&self, stack_id: i64) -> bool {
        self.in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&stack_id)
    }

    /// Claims a stack for one operation, or `None` if one is running.
    #[must_use]
    pub fn claim(&self, stack_id: i64) -> Option<Slot> {
        self.in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(stack_id)
            .then(|| Slot {
                in_flight: Arc::clone(&self.in_flight),
                stack_id,
            })
    }

    /// Records the attempt, starts it in the background, and returns at once.
    pub async fn start(
        &self,
        stack: &RegisteredStack,
        action: Action,
        trigger: Trigger,
    ) -> Result<Deployment, RunError> {
        // Held until the operation is over, however it ends.
        let slot = self.claim(stack.id).ok_or(RunError::Busy)?;
        let deployment = self
            .store
            .deployment_start(stack.id, action, trigger)
            .await?;

        let _ = self.events.send(ServerEvent::DeploymentStarted {
            stack_id: stack.id,
            deployment_id: deployment.id,
            action,
        });

        let this = self.clone();
        let stack = stack.clone();
        let id = deployment.id;
        tokio::spawn(async move {
            use futures::FutureExt as _;
            let _slot = slot;
            // A panic is still an outcome to record, not a deployment left
            // marked running until the next restart.
            let outcome = std::panic::AssertUnwindSafe(this.execute(&stack, action, id))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| {
                    Outcome::failed("GhostDock failed while running this.".to_owned())
                });
            this.finish(stack.id, id, outcome).await;
        });

        Ok(deployment)
    }

    async fn execute(
        &self,
        stack: &RegisteredStack,
        action: Action,
        deployment_id: i64,
    ) -> Outcome {
        let operation = match action {
            Action::Deploy => return self.deploy(stack, deployment_id).await,
            Action::Stop => Operation::Stop,
            Action::Restart => Operation::Restart,
            Action::Remove => Operation::TakeDown,
        };
        self.operate(stack, operation, deployment_id).await
    }

    /// Brings the stack to what its source says now: the one action that
    /// fetches, and the one that moves the recorded commit.
    async fn deploy(&self, stack: &RegisteredStack, deployment_id: i64) -> Outcome {
        let prepared = match self.prepare(stack).await {
            Ok(prepared) => prepared,
            // Failing to reach the repository, or to read the file it names,
            // is a failed deploy with git's own explanation -- not a silent
            // fallback to whatever was deployed last time.
            Err(e) => return Outcome::failed(e.to_string()),
        };
        let commit = prepared.commit.clone();
        let project = prepared.project(&stack.slug);

        // Validate before touching anything. A compose file that cannot be
        // resolved should fail with compose's own message rather than half
        // apply and leave the stack in an in-between state.
        let check = self
            .run(command::config(&project), deployment_id, false)
            .await;
        match check {
            Ok(outcome) if !outcome.success => {
                return Outcome {
                    success: false,
                    exit_code: outcome.exit_code,
                    log: outcome.output,
                    commit,
                };
            }
            Err(e) => return Outcome::failed(e.to_string()),
            Ok(_) => {}
        }

        let argv = command::up(&project, Pull::Always, WAIT_TIMEOUT_SECS);
        Self::outcome(self.run(argv, deployment_id, true).await, commit)
    }

    /// Stops, restarts or takes down what was last deployed.
    ///
    /// Never fetches: the repository may have moved on, and doing any of
    /// these must not quietly change what the stack runs, nor make it look
    /// up to date when it is not.
    async fn operate(
        &self,
        stack: &RegisteredStack,
        operation: Operation,
        deployment_id: i64,
    ) -> Outcome {
        let argv = match self.existing(stack).await {
            Ok(Existing::Files(prepared)) => {
                let project = prepared.project(&stack.slug);
                match operation {
                    Operation::Stop => command::stop(&project),
                    Operation::Restart => command::restart(&project),
                    Operation::TakeDown => command::down(&project),
                }
            }
            Ok(Existing::Name(dir)) => match operation {
                Operation::Stop => command::stop_by_name(&stack.slug, &dir),
                Operation::Restart => command::restart_by_name(&stack.slug, &dir),
                Operation::TakeDown => command::down_by_name(&stack.slug, &dir),
            },
            Err(e) => return Outcome::failed(e.to_string()),
        };
        Self::outcome(self.run(argv, deployment_id, true).await, None)
    }

    fn outcome(run: Result<compose::Outcome, compose::Error>, commit: Option<String>) -> Outcome {
        match run {
            Ok(outcome) => {
                let note = if outcome.timed_out {
                    format!(
                        "\nghostdock stopped this after {} minutes.",
                        COMMAND_TIMEOUT.as_secs() / 60
                    )
                } else {
                    String::new()
                };
                Outcome {
                    success: outcome.success,
                    exit_code: outcome.exit_code,
                    log: format!("{}{note}", outcome.output),
                    commit,
                }
            }
            Err(e) => Outcome::failed(e.to_string()),
        }
    }

    /// The files a stack was last deployed from, as they are on disk.
    ///
    /// Falls back to the project's name when there are none (a stack
    /// registered but never deployed from here, or whose checkout is gone),
    /// unless compose would then read some other file in their place.
    async fn existing(&self, stack: &RegisteredStack) -> Result<Existing, RunError> {
        compose::slug::validate(&stack.slug).map_err(compose::Error::from)?;
        let dir = self.compose.project_dir(&stack.slug);
        let env_file = Some(dir.join(compose::ENV_FILE)).filter(|p| p.is_file());

        let found = match stack.git.as_ref() {
            // The stored file is the stack, and writing it out fetches
            // nothing, so a stack never deployed from here gets one.
            None if !dir.join(COMPOSE_FILE).is_file() => {
                return self.prepare(stack).await.map(Existing::Files);
            }
            None => Some(Prepared {
                project_dir: dir.clone(),
                compose_file: COMPOSE_FILE.to_owned(),
                env_file,
                commit: None,
            }),
            Some(git) => {
                let repo_dir = dir.join("repo");
                gitsync::resolve_in_repo(&repo_dir, &git.compose_path)
                    .ok()
                    .filter(|path| repo_dir.join(".git").exists() && path.is_file())
                    .map(|path| Prepared {
                        project_dir: path.parent().unwrap_or(&repo_dir).to_path_buf(),
                        compose_file: path.file_name().map_or_else(
                            || COMPOSE_FILE.to_owned(),
                            |name| name.to_string_lossy().into_owned(),
                        ),
                        env_file,
                        commit: None,
                    })
            }
        };
        if let Some(prepared) = found {
            return Ok(Existing::Files(prepared));
        }

        if let Some(stray) = compose::default_file_above(&dir).await {
            return Err(RunError::NoFiles(stray));
        }
        Ok(Existing::Name(dir))
    }

    /// Puts the stack's compose file and environment where compose can read
    /// them.
    async fn prepare(&self, stack: &RegisteredStack) -> Result<Prepared, RunError> {
        let vars = self.store.stack_env_get(stack.id).await?;

        let Some(git) = stack.git.as_ref() else {
            let yaml = self
                .store
                .stack_compose_yaml(stack.id)
                .await?
                .unwrap_or_default();
            let dir = self.compose.materialise(&stack.slug, &yaml, &vars).await?;
            let env_file = (!vars.is_empty()).then(|| dir.join(compose::ENV_FILE));
            return Ok(Prepared {
                project_dir: dir,
                compose_file: COMPOSE_FILE.to_owned(),
                env_file,
                commit: None,
            });
        };

        let credential = credential_for(&self.store, git.repo_id).await?;
        let repo_dir = self.compose.project_dir(&stack.slug).join("repo");
        let commit = self
            .git
            .sync(&repo_dir, &git.repo_url, &git.git_ref, credential.as_ref())
            .await?;

        // Run against the file where it lives in the repository, so relative
        // paths inside it resolve the way its author intended. Copying it
        // elsewhere would quietly break every `./config:/config`.
        let path = gitsync::resolve_in_repo(&repo_dir, &git.compose_path)?;
        if !path.is_file() {
            return Err(RunError::Git(gitsync::Error::NotInRepo(
                git.compose_path.clone(),
            )));
        }
        let project_dir = path.parent().unwrap_or(&repo_dir).to_path_buf();
        let compose_file = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| COMPOSE_FILE.to_owned());

        // Outside the working tree: the next `git clean` would remove it, and
        // a repository may ship its own `.env`.
        let env_file = self.compose.write_env_file(&stack.slug, &vars).await?;

        Ok(Prepared {
            project_dir,
            compose_file,
            env_file,
            commit: Some(commit),
        })
    }

    /// Checks a repository out at `git_ref` and lists the tracked files
    /// matching `pattern`, sorted. Returns the commit it looked at.
    ///
    /// Uses a checkout of its own per repository, apart from any stack's, so
    /// looking never disturbs what a stack deploys from. One discovery runs
    /// at a time; two at once would race over the same checkout.
    pub async fn discover(
        &self,
        repo_id: i64,
        url: &str,
        git_ref: &str,
        pattern: &domain::glob::Pattern,
    ) -> Result<(String, Vec<String>), RunError> {
        let _one_at_a_time = self.discovering.lock().await;
        let credential = credential_for(&self.store, repo_id).await?;
        let dir = self
            .compose
            .project_dir(DISCOVERY_DIR)
            .join(repo_id.to_string());
        let commit = self
            .git
            .sync(&dir, url, git_ref, credential.as_ref())
            .await?;
        let mut files: Vec<String> = self
            .git
            .list_files(&dir)
            .await?
            .into_iter()
            .filter(|path| pattern.matches(path))
            .collect();
        files.sort();
        Ok((commit, files))
    }

    /// Runs one invocation, optionally forwarding its output to subscribers.
    ///
    /// `config` output is not streamed: it is a JSON document, not progress,
    /// and pushing it at the UI line by line would be noise.
    async fn run(
        &self,
        argv: Vec<String>,
        deployment_id: i64,
        stream: bool,
    ) -> Result<compose::Outcome, compose::Error> {
        let (sink, forwarder) = if stream {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let events = self.events.clone();
            let forwarder = tokio::spawn(async move {
                while let Some(line) = rx.recv().await {
                    let _ = events.send(ServerEvent::DeploymentOutput {
                        deployment_id,
                        line,
                    });
                }
            });
            (Some(tx), Some(forwarder))
        } else {
            (None, None)
        };

        let result = self.compose.run(&argv, COMMAND_TIMEOUT, sink).await;
        // The sender went with `run`, so this ends once every line is sent:
        // a client never hears "finished" before the last of the output.
        if let Some(forwarder) = forwarder {
            let _ = forwarder.await;
        }
        result
    }

    /// Removes the checkout discovery keeps for a repository.
    pub async fn forget_discovery(&self, repo_id: i64) -> std::io::Result<()> {
        // Not while a discovery is using it.
        let _one_at_a_time = self.discovering.lock().await;
        let dir = self
            .compose
            .project_dir(DISCOVERY_DIR)
            .join(repo_id.to_string());
        match tokio::fs::remove_dir_all(&dir).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Removes a forgotten stack's `.env`, which holds its secrets in plain
    /// text. Everything else in its directory stays: its containers may
    /// still be running and using it.
    pub async fn forget_env(&self, slug: &str) -> Result<(), compose::Error> {
        self.compose.remove_env_file(slug).await
    }

    async fn finish(&self, stack_id: i64, deployment_id: i64, outcome: Outcome) {
        if let Err(e) = self
            .store
            .deployment_finish(
                deployment_id,
                outcome.success,
                outcome.exit_code,
                &outcome.log,
                outcome.commit.as_deref(),
            )
            .await
        {
            tracing::error!(error = %e, deployment_id, "could not record deployment outcome");
        }

        // Only a successful deploy moves the stack's recorded commit; a
        // failed one must not make the stack look up to date.
        if outcome.success
            && let Some(commit) = outcome.commit.as_deref()
            && let Err(e) = self.store.stack_set_last_commit(stack_id, commit).await
        {
            tracing::error!(error = %e, stack_id, "could not record the deployed commit");
        }

        match self.store.deployment_detail(deployment_id).await {
            Ok(Some(detail)) => {
                let _ = self.events.send(ServerEvent::DeploymentFinished {
                    deployment: detail.deployment,
                });
            }
            Ok(None) => tracing::error!(deployment_id, "deployment vanished while running"),
            Err(e) => tracing::error!(error = %e, "could not read back deployment"),
        }
    }
}

/// The credential for a repository, if one is configured.
pub(crate) async fn credential_for(
    store: &Store,
    repo_id: i64,
) -> Result<Option<Credential>, store::Error> {
    let Some(credential_id) = store
        .repo_by_id(repo_id)
        .await?
        .and_then(|repo| repo.credential_id)
    else {
        return Ok(None);
    };
    Ok(store
        .credential_secret(credential_id)
        .await?
        .map(|(username, secret)| Credential { username, secret }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_panic_while_running_still_frees_the_stack() {
        let store = Store::open_in_memory().await.expect("store");
        let runner = Runner::new(Compose::new("/nonexistent"), store);

        let slot = runner.claim(7).expect("free");
        assert!(runner.is_busy(7));
        assert!(runner.claim(7).is_none(), "one operation at a time");

        let crashed = tokio::spawn(async move {
            let _slot = slot;
            panic!("an operation failed badly");
        })
        .await;
        assert!(crashed.is_err());
        assert!(
            !runner.is_busy(7),
            "a crashed operation must not leave its stack busy for good"
        );
    }
}
