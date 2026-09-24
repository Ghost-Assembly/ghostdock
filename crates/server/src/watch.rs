//! Relays the daemon's container events to connected clients.
//!
//! This is what makes the board live: a container that crashes, restarts or
//! turns unhealthy shows up without anyone refreshing, whoever caused it.

use std::time::Duration;

use futures::StreamExt as _;
use shared::event::ServerEvent;

use crate::runner::Runner;

const FIRST_RETRY: Duration = Duration::from_secs(1);
const LONGEST_RETRY: Duration = Duration::from_secs(30);

/// Watches for as long as the process runs, resubscribing with backoff when
/// the daemon goes away (a restart, an upgrade).
pub fn spawn(docker: docker::Client, runner: Runner) {
    tokio::spawn(async move {
        let mut retry = FIRST_RETRY;
        loop {
            let mut changes = Box::pin(docker.container_changes());
            while let Some(change) = changes.next().await {
                match change {
                    Ok(change) => {
                        retry = FIRST_RETRY;
                        runner.publish(ServerEvent::ContainerChanged { change });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "lost the Docker event stream");
                        break;
                    }
                }
            }
            tokio::time::sleep(retry).await;
            retry = (retry * 2).min(LONGEST_RETRY);
        }
    });
}
