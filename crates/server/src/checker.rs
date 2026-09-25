//! Finding out what is waiting, on a timer.
//!
//! Two questions per stack: has the tracked ref moved, and have any of the
//! images it runs been republished. Both are cheap enough to ask repeatedly;
//! neither pulls anything.

use std::collections::BTreeSet;
use std::time::Duration;

use gitsync::Git;
use shared::container::Container;
use shared::deployment::{Action, RegisteredStack, Trigger};
use shared::update::{ImageStatus, UpdateStatus};
use store::Store;

use crate::runner::Runner;

/// How often every stack is checked.
///
/// Registries are the binding constraint, not Git: anonymous Docker Hub
/// allows a limited number of manifest requests per IP per six hours, shared
/// with real pulls. Hourly across a few dozen stacks stays well inside that.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Waited before the first sweep.
///
/// Startup is when the daemon is busiest and a user is most likely watching;
/// a burst of registry traffic then buys nothing.
const STARTUP_DELAY: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct Checker {
    store: Store,
    docker: Option<docker::Client>,
    git: Git,
    registry: registry::Client,
    runner: Runner,
    host_id: i64,
}

impl std::fmt::Debug for Checker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Checker").finish_non_exhaustive()
    }
}

impl Checker {
    pub fn new(store: Store, docker: Option<docker::Client>, runner: Runner, host_id: i64) -> Self {
        Self {
            store,
            docker,
            git: Git::new(),
            registry: registry::Client::new(),
            runner,
            host_id,
        }
    }

    /// Runs a sweep on a timer until the process ends.
    pub fn spawn(self, interval: Duration) {
        tokio::spawn(async move {
            tokio::time::sleep(STARTUP_DELAY).await;
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                self.sweep().await;
            }
        });
    }

    /// Checks every stack, then applies anything set to apply itself.
    pub async fn sweep(&self) {
        let stacks = match self.store.stacks_list(self.host_id).await {
            Ok(stacks) => stacks,
            Err(e) => {
                tracing::error!(error = %e, "could not list stacks to check");
                return;
            }
        };

        // Once for the whole pass rather than once a stack: the daemon's
        // list is the same for all of them.
        let containers = self.containers().await;
        for stack in stacks {
            let status = self.check_with(&stack, containers.as_deref()).await;
            if status.has_update() {
                self.maybe_apply(&stack).await;
            }
        }
    }

    /// Checks one stack and records the result.
    pub async fn check(&self, stack: &RegisteredStack) -> UpdateStatus {
        let containers = self.containers().await;
        self.check_with(stack, containers.as_deref()).await
    }

    /// Every container on the host, or `None` without a daemon to ask.
    async fn containers(&self) -> Option<Vec<Container>> {
        self.docker.as_ref()?.list_containers().await.ok()
    }

    /// [`Self::check`], given the host's containers.
    async fn check_with(
        &self,
        stack: &RegisteredStack,
        containers: Option<&[Container]>,
    ) -> UpdateStatus {
        let (remote_commit, git_error) = self.check_git(stack).await;
        let images = self.check_images(stack, containers).await;

        if let Err(e) = self
            .store
            .update_record(
                stack.id,
                remote_commit.as_deref(),
                git_error.as_deref(),
                &images,
            )
            .await
        {
            tracing::error!(error = %e, stack = %stack.slug, "could not record a check");
        }

        self.store
            .update_status(stack.id)
            .await
            .unwrap_or_else(|_| UpdateStatus::default())
    }

    /// Asks the remote what the tracked ref points at.
    async fn check_git(&self, stack: &RegisteredStack) -> (Option<String>, Option<String>) {
        let Some(git) = stack.git.as_ref() else {
            return (None, None);
        };

        let credential = crate::runner::credential_for(&self.store, git.repo_id)
            .await
            .ok()
            .flatten();
        match self
            .git
            .remote_head(&git.repo_url, &git.git_ref, credential.as_ref())
            .await
        {
            Ok(commit) => (Some(commit), None),
            // Reported as a failed check rather than as "no update", which
            // would be indistinguishable from everything being current.
            Err(e) => (None, Some(e.to_string())),
        }
    }

    /// Compares what each running image is against what its tag points at.
    async fn check_images(
        &self,
        stack: &RegisteredStack,
        containers: Option<&[Container]>,
    ) -> Vec<ImageStatus> {
        let (Some(docker), Some(containers)) = (self.docker.as_ref(), containers) else {
            return Vec::new();
        };

        // One entry per distinct image: several services commonly share one,
        // and asking a registry the same question twice spends rate limit for
        // an answer already held.
        let images: BTreeSet<String> = containers
            .iter()
            .filter(|c| c.compose.as_ref().is_some_and(|m| m.project == stack.slug))
            .map(|c| c.image.clone())
            .filter(|image| !image.is_empty())
            .collect();

        let mut statuses = Vec::new();
        for image in images {
            statuses.push(self.check_image(docker, &image).await);
        }
        statuses
    }

    async fn check_image(&self, docker: &docker::Client, image: &str) -> ImageStatus {
        let reference = match registry::reference::parse(image) {
            Ok(reference) => reference,
            Err(e) => {
                return ImageStatus {
                    image: image.to_owned(),
                    running: None,
                    available: None,
                    error: Some(e.to_string()),
                };
            }
        };

        // An image pinned to a digest cannot drift, so asking about it spends
        // a rate-limit token for an answer that is knowable in advance.
        if reference.is_pinned() {
            return ImageStatus {
                image: image.to_owned(),
                running: reference.digest.clone(),
                available: reference.digest.clone(),
                error: None,
            };
        }

        let running = docker.image_digest(image).await.ok().flatten();
        match self.registry.digest(&reference).await {
            Ok(available) => ImageStatus {
                image: image.to_owned(),
                running,
                available: Some(available),
                error: None,
            },
            Err(e) => ImageStatus {
                image: image.to_owned(),
                running,
                available: None,
                error: Some(e.to_string()),
            },
        }
    }

    /// Deploys a stack that is set to apply updates on its own.
    async fn maybe_apply(&self, stack: &RegisteredStack) {
        match self.store.stack_auto_apply(stack.id).await {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => {
                tracing::error!(error = %e, "could not read the auto-apply setting");
                return;
            }
        }

        match self
            .runner
            .start(stack, Action::Deploy, Trigger::Schedule)
            .await
        {
            Ok(deployment) => tracing::info!(
                stack = %stack.slug,
                deployment = deployment.id,
                "applying an update automatically"
            ),
            // Busy is ordinary: something is already running for this stack,
            // and the next sweep will find it again if it still matters.
            Err(e) => tracing::debug!(error = %e, stack = %stack.slug, "not applying now"),
        }
    }
}
