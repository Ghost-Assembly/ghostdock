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
        self.in_flight.lock().expect("lock").contains(&stack_id)
    }

    /// Records the attempt, starts it in the background, and returns at once.
    pub async fn start(
        &self,
        stack: &RegisteredStack,
        action: Action,
        trigger: Trigger,
    ) -> Result<Deployment, RunError> {
        if !self.in_flight.lock().expect("lock").insert(stack.id) {
            return Err(RunError::Busy);
        }

        // From here on the slot is held, so every path must release it.
        let deployment = match self.store.deployment_start(stack.id, action, trigger).await {
            Ok(d) => d,
            Err(e) => {
                self.release(stack.id);
                return Err(e.into());
            }
        };

        let _ = self.events.send(ServerEvent::DeploymentStarted {
            stack_id: stack.id,
            deployment_id: deployment.id,
            action,
        });

        let this = self.clone();
        let stack = stack.clone();
        let id = deployment.id;
        tokio::spawn(async move {
            let outcome = this.execute(&stack, action, id).await;
            this.finish(stack.id, id, outcome).await;
            this.release(stack.id);
        });

        Ok(deployment)
    }

    fn release(&self, stack_id: i64) {
        self.in_flight.lock().expect("lock").remove(&stack_id);
    }

    /// Returns `(success, exit_code, log, commit)`.
    async fn execute(
        &self,
        stack: &RegisteredStack,
        action: Action,
        deployment_id: i64,
    ) -> Outcome {
        let prepared = match self.prepare(stack).await {
            Ok(prepared) => prepared,
            // Failing to reach the repository, or to read the file it names,
            // is a failed deploy with git's own explanation -- not a silent
            // fallback to whatever was deployed last time.
            Err(e) => return Outcome::failed(e.to_string()),
        };
        let commit = prepared.commit.clone();

        let project = Project {
            name: &stack.slug,
            dir: &prepared.project_dir,
            file: &prepared.compose_file,
            env_file: prepared.env_file.as_deref(),
        };

        // Validate before touching anything. A compose file that cannot be
        // resolved should fail with compose's own message rather than half
        // apply and leave the stack in an in-between state.
        if matches!(action, Action::Deploy) {
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
        }

        let argv = match action {
            Action::Deploy => command::up(&project, Pull::Always, WAIT_TIMEOUT_SECS),
            Action::Stop => command::stop(&project),
            Action::Restart => command::restart(&project),
            Action::Remove => command::down(&project),
        };

        match self.run(argv, deployment_id, true).await {
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

        let credential = self.credential_for(git.repo_id).await?;
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
        let credential = self.credential_for(repo_id).await?;
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

    /// The credential for a repository, if one is configured.
    async fn credential_for(&self, repo_id: i64) -> Result<Option<Credential>, RunError> {
        let Some(repo) = self.store.repo_by_id(repo_id).await? else {
            return Ok(None);
        };
        let Some(credential_id) = repo.credential_id else {
            return Ok(None);
        };
        Ok(self
            .store
            .credential_secret(credential_id)
            .await?
            .map(|(username, secret)| Credential { username, secret }))
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
        let sink = stream.then(|| {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let events = self.events.clone();
            tokio::spawn(async move {
                while let Some(line) = rx.recv().await {
                    let _ = events.send(ServerEvent::DeploymentOutput {
                        deployment_id,
                        line,
                    });
                }
            });
            tx
        });

        self.compose.run(&argv, COMMAND_TIMEOUT, sink).await
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
