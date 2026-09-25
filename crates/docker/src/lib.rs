//! Docker daemon access via bollard.
//!
//! ARCHITECTURAL INVARIANT: READ PATH — list, inspect, logs, stats, events.
//! Every change to a stack goes through the `compose` crate so that
//! `docker compose` remains the single source of truth for what a stack is.
//!
//! The one exception is cleanup: removing unused images and stopped
//! containers nobody manages. There is no Compose command for either, and
//! neither is part of a live stack. Exec is the other thing this crate
//! starts, and it changes nothing about a stack's definition.

pub mod cleanup;
pub mod events;
pub mod logs;
pub mod map;
pub mod shell;
pub mod stats;

use bollard::query_parameters::ListContainersOptionsBuilder;
use shared::container::Container;
use shared::host::HostInfo;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not reach the Docker daemon: {0}")]
    Unreachable(#[source] bollard::errors::Error),
    #[error("Docker API call failed: {0}")]
    Api(#[from] bollard::errors::Error),
}

/// What kind of failure an [`Error`] is, for a caller deciding how to
/// report it without knowing bollard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The daemon has no such container, image or exec.
    NotFound,
    /// The request conflicts with the object's state, such as running a
    /// command in a container that is not running.
    Conflict,
    /// The daemon could not be reached or failed to answer.
    Unavailable,
}

impl Error {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Api(bollard::errors::Error::DockerResponseServerError {
                status_code: 404,
                ..
            }) => ErrorKind::NotFound,
            Self::Api(bollard::errors::Error::DockerResponseServerError {
                status_code: 409,
                ..
            }) => ErrorKind::Conflict,
            _ => ErrorKind::Unavailable,
        }
    }

    /// The daemon's own explanation, when it gave one.
    #[must_use]
    pub fn daemon_message(&self) -> Option<&str> {
        match self {
            Self::Api(bollard::errors::Error::DockerResponseServerError { message, .. }) => {
                Some(message.as_str())
            }
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Most output kept from one command, in bytes. Past it the command's
/// output is cut off: an endless command must not become an endless reply.
pub const RUN_OUTPUT_LIMIT: usize = 256 * 1024;

/// A connection to one Docker daemon.
///
/// Cloning is cheap; bollard shares the underlying connection pool.
#[derive(Debug, Clone)]
pub struct Client {
    inner: bollard::Docker,
}

impl Client {
    /// Connects using the daemon's conventional local defaults.
    ///
    /// Honours `DOCKER_HOST` when set, and discovers a rootless Podman
    /// socket otherwise, so the same binary works on both runtimes.
    pub fn connect() -> Result<Self> {
        let inner = bollard::Docker::connect_with_defaults().map_err(Error::Unreachable)?;
        Ok(Self { inner })
    }

    /// Container changes as the daemon reports them, from now on.
    ///
    /// Ends when the connection does, after yielding the error; the caller
    /// decides when to subscribe again. Events that happen in between are
    /// not replayed, so a caller should re-read state after reconnecting.
    pub fn container_changes(
        &self,
    ) -> impl futures::Stream<Item = Result<shared::event::ContainerChange>> + Send + use<> {
        use futures::StreamExt as _;
        let options = bollard::query_parameters::EventsOptions {
            filters: Some(events::filters()),
            ..Default::default()
        };
        self.inner
            .events(Some(options))
            .filter_map(|event| async move {
                match event {
                    Ok(message) => events::to_change(message).map(Ok),
                    Err(e) => Some(Err(Error::Api(e))),
                }
            })
    }

    /// Daemon version string.
    pub async fn version(&self) -> Result<Option<String>> {
        Ok(self.inner.version().await?.version)
    }

    /// Every container on the host, running or not.
    pub async fn list_containers(&self) -> Result<Vec<Container>> {
        let options = ListContainersOptionsBuilder::new().all(true).build();
        let summaries = self.inner.list_containers(Some(options)).await?;
        Ok(summaries.into_iter().map(map::to_container).collect())
    }

    /// The mounts on a container, by id or name.
    ///
    /// Used to inspect GhostDock's own container. Returns `None` when there is
    /// no such container, which is the ordinary case when GhostDock is not
    /// running in one at all.
    pub async fn mounts_of(&self, container: &str) -> Result<Option<Vec<domain::contract::Mount>>> {
        let inspected = match self.inner.inspect_container(container, None).await {
            Ok(inspected) => inspected,
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };

        Ok(Some(
            inspected
                .mounts
                .unwrap_or_default()
                .into_iter()
                .filter_map(|m| {
                    Some(domain::contract::Mount {
                        source: m.source?,
                        destination: m.destination?,
                    })
                })
                .collect(),
        ))
    }

    /// Runs one command with `sh -c` and collects what it prints.
    ///
    /// Waits at most `wait`. A command still running then is reported as
    /// timed out and left to finish on its own: the daemon has no way to
    /// stop an exec. Output past [`RUN_OUTPUT_LIMIT`] is cut off, and
    /// closing the stream at that point ends a command that only prints,
    /// since its next write fails.
    pub async fn run_command(
        &self,
        container: &str,
        command: &str,
        wait: std::time::Duration,
    ) -> Result<shared::logs::CommandResult> {
        use bollard::exec::{CreateExecOptions, StartExecResults};
        use futures::StreamExt as _;

        let created = self
            .inner
            .create_exec(
                container,
                CreateExecOptions {
                    cmd: Some(vec!["sh".to_owned(), "-c".to_owned(), command.to_owned()]),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    tty: Some(false),
                    ..Default::default()
                },
            )
            .await?;

        let mut output = Vec::new();
        let mut bytes = 0;
        let mut truncated = false;
        if let StartExecResults::Attached {
            output: mut stream, ..
        } = self.inner.start_exec(&created.id, None).await?
        {
            let collect = async {
                while let Some(chunk) = stream.next().await {
                    for line in logs::to_lines(&chunk?) {
                        bytes += line.text.len();
                        if bytes > RUN_OUTPUT_LIMIT {
                            truncated = true;
                            return Ok::<_, Error>(());
                        }
                        output.push(line);
                    }
                }
                Ok(())
            };
            match tokio::time::timeout(wait, collect).await {
                Ok(done) => done?,
                Err(_) => {
                    return Ok(shared::logs::CommandResult {
                        exit_code: None,
                        output,
                        truncated,
                        timed_out: true,
                    });
                }
            }
        }

        let exit_code = if truncated {
            None
        } else {
            self.inner.inspect_exec(&created.id).await?.exit_code
        };
        Ok(shared::logs::CommandResult {
            exit_code,
            output,
            truncated,
            timed_out: false,
        })
    }

    /// A container's counters about once a second, for as long as it runs.
    /// Ends when the container stops (the daemon would otherwise stream
    /// zeroed figures for it indefinitely); the caller keeps the time between
    /// readings, since the daemon's timestamp type varies by build.
    pub fn stats(
        &self,
        container: &str,
    ) -> impl futures::Stream<Item = Result<domain::metrics::Counters>> + Send + use<> {
        use futures::StreamExt as _;
        let options = bollard::query_parameters::StatsOptionsBuilder::new()
            .stream(true)
            .build();
        self.inner
            .stats(container, Some(options))
            .map(|message| match message {
                Ok(s) => stats::to_counters(&s).map(Ok),
                Err(e) => Some(Err(Error::Api(e))),
            })
            .take_while(|item| std::future::ready(item.is_some()))
            .filter_map(std::future::ready)
    }

    /// Opens a shell inside a running container.
    ///
    /// Started without a TTY on purpose: the daemon then keeps stdout and
    /// stderr apart and emits no escape sequences, which is what lets a
    /// line-oriented client be honest rather than a terminal that renders
    /// badly. The cost is that cursor-addressed programs will not work.
    pub async fn start_shell(&self, container: &str, shell: &str) -> Result<shell::Shell> {
        use bollard::exec::{CreateExecOptions, StartExecResults};

        let created = self
            .inner
            .create_exec(
                container,
                CreateExecOptions {
                    cmd: Some(vec![shell.to_owned()]),
                    attach_stdin: Some(true),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    tty: Some(false),
                    ..Default::default()
                },
            )
            .await?;

        match self.inner.start_exec(&created.id, None).await? {
            StartExecResults::Attached { output, input } => Ok(shell::Shell::new(output, input)),
            // The daemon only detaches when asked to, which this never does.
            StartExecResults::Detached => {
                Err(Error::Api(bollard::errors::Error::DockerStreamError {
                    error: "the daemon detached a shell that asked to attach".to_owned(),
                }))
            }
        }
    }

    /// What a cleanup would remove, without removing anything.
    /// What could be removed. `registered` names the compose projects GhostDock
    /// manages, whose stopped containers are never offered.
    pub async fn cleanup_preview(
        &self,
        registered: &std::collections::HashSet<String>,
    ) -> Result<shared::cleanup::CleanupPreview> {
        let images = self
            .inner
            .list_images(None::<bollard::query_parameters::ListImagesOptions>)
            .await?;
        // Every container, not only running ones: a stopped container still
        // protects the image underneath it.
        let containers = self
            .inner
            .list_containers(Some(
                bollard::query_parameters::ListContainersOptionsBuilder::new()
                    .all(true)
                    .build(),
            ))
            .await?;

        let mut preview = cleanup::preview(&images, &containers);
        (preview.leftover, preview.standalone) =
            cleanup::stopped_containers(&containers, registered);
        Ok(preview)
    }

    /// Removes stopped containers. Never forced, so the daemon refuses any
    /// that started since the preview, and never with their volumes.
    pub async fn remove_containers(
        &self,
        containers: &[shared::cleanup::StoppedContainer],
    ) -> shared::cleanup::CleanupResult {
        let mut result = shared::cleanup::CleanupResult::default();
        let options = bollard::query_parameters::RemoveContainerOptionsBuilder::new()
            .force(false)
            .v(false)
            .build();
        for container in containers {
            match self
                .inner
                .remove_container(&container.id, Some(options.clone()))
                .await
            {
                Ok(()) => result.removed.push(container.name.clone()),
                Err(e) => {
                    tracing::debug!(container = %container.name, error = %e, "container kept");
                    result.kept.push(container.name.clone());
                }
            }
        }
        result
    }

    /// Removes images by id, reporting what went and what did not.
    ///
    /// One at a time rather than a bulk prune, so a single image the daemon
    /// refuses does not abandon the rest, and so the result can say exactly
    /// what happened.
    pub async fn remove_images(
        &self,
        images: &[shared::cleanup::UnusedImage],
    ) -> shared::cleanup::CleanupResult {
        let mut result = shared::cleanup::CleanupResult::default();

        for image in images {
            match self
                .inner
                .remove_image(
                    &image.id,
                    None::<bollard::query_parameters::RemoveImageOptions>,
                    None,
                )
                .await
            {
                Ok(_) => {
                    result.reclaimed_bytes += image.size_bytes;
                    result.removed.push(image.label());
                }
                // An image can become used between the preview and the
                // request; that is ordinary and not worth failing over.
                Err(e) => {
                    tracing::debug!(image = %image.label(), error = %e, "image kept");
                    result.kept.push(image.label());
                }
            }
        }

        result
    }

    /// New output from a container as it is written, until it stops.
    ///
    /// `since` is a Unix time in seconds; lines from that second onward are
    /// included, so a caller continuing from a snapshot should drop lines it
    /// already has by timestamp. Without it, only lines written from now on.
    pub fn follow_logs(
        &self,
        id: &str,
        since: Option<i64>,
    ) -> impl futures::Stream<Item = Result<shared::logs::LogLine>> + Send + use<> {
        use futures::StreamExt as _;

        let mut options = bollard::query_parameters::LogsOptionsBuilder::new()
            .stdout(true)
            .stderr(true)
            .timestamps(true)
            .follow(true);
        options = match since.and_then(|s| i32::try_from(s).ok()) {
            Some(since) => options.since(since).tail("all"),
            None => options.tail("0"),
        };

        self.inner
            .logs(id, Some(options.build()))
            .flat_map(|chunk| {
                let items: Vec<Result<shared::logs::LogLine>> = match chunk {
                    Ok(chunk) => logs::to_lines(&chunk).into_iter().map(Ok).collect(),
                    Err(e) => vec![Err(Error::Api(e))],
                };
                futures::stream::iter(items)
            })
    }

    /// Recent output from a container.
    ///
    /// A bounded tail rather than everything: a long-running container's log
    /// can be hundreds of megabytes, and sending it to a phone to find the
    /// last twenty lines helps nobody.
    pub async fn container_logs(&self, id: &str, tail: usize) -> Result<shared::logs::Logs> {
        use futures::StreamExt as _;

        let options = bollard::query_parameters::LogsOptionsBuilder::new()
            .stdout(true)
            .stderr(true)
            .timestamps(true)
            .tail(&tail.to_string())
            .build();

        let mut stream = self.inner.logs(id, Some(options));
        let mut lines = Vec::new();
        while let Some(chunk) = stream.next().await {
            lines.extend(logs::to_lines(&chunk?));
        }

        Ok(shared::logs::Logs {
            container: id.to_owned(),
            // The daemon gives no total, so "there may be more" is inferred
            // from having received exactly what was asked for.
            truncated: lines.len() >= tail,
            lines,
        })
    }

    /// The registry digest of the image a container is running.
    ///
    /// Not the local image id: that is a content hash of the unpacked image
    /// and has nothing to do with what a registry would report. The digest
    /// wanted here is the one recorded when the image was pulled, which is
    /// what a registry's manifest returns for the same tag.
    ///
    /// `None` when the image was built locally or loaded from a file, since
    /// it then has no registry digest and cannot meaningfully drift.
    pub async fn image_digest(&self, image: &str) -> Result<Option<String>> {
        let inspected = match self.inner.inspect_image(image).await {
            Ok(inspected) => inspected,
            // An image that is gone is not an error worth failing a sweep
            // over; it simply cannot be compared.
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        Ok(map::repo_digest(inspected.repo_digests.as_deref(), image))
    }

    /// Host overview. Never fails on an unreachable daemon: the reason is
    /// reported in the payload so the UI can render the page and say why it
    /// is empty, rather than showing an error instead of the page.
    pub async fn host_info(&self, id: i64, name: String) -> HostInfo {
        match self.inner.info().await {
            Ok(info) => HostInfo {
                id,
                name,
                server_version: info.server_version,
                containers_running: u64::try_from(info.containers_running.unwrap_or(0))
                    .unwrap_or(0),
                containers_total: u64::try_from(info.containers.unwrap_or(0)).unwrap_or(0),
                images: u64::try_from(info.images.unwrap_or(0)).unwrap_or(0),
                unreachable_reason: None,
                problems: Vec::new(),
            },
            Err(e) => HostInfo {
                id,
                name,
                server_version: None,
                containers_running: 0,
                containers_total: 0,
                images: 0,
                unreachable_reason: Some(e.to_string()),
                problems: Vec::new(),
            },
        }
    }
}
