//! Events pushed to clients over SSE.
//!
//! One stream carries every kind of event. Browsers cap concurrent
//! connections per origin, so a stream per feature fails on the device that
//! matters most.

use serde::{Deserialize, Serialize};

use crate::deployment::{Action, Deployment};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    /// A stack operation has begun.
    DeploymentStarted {
        stack_id: i64,
        deployment_id: i64,
        action: Action,
    },
    /// One line of output, as compose produced it.
    DeploymentOutput { deployment_id: i64, line: String },
    /// The operation finished, one way or the other.
    ///
    /// The deployment is flattened rather than nested, and carries its own
    /// `stack_id`; adding another here produced a duplicate JSON key, which
    /// different parsers resolve differently.
    DeploymentFinished {
        #[serde(flatten)]
        deployment: Deployment,
    },
    /// Something happened to a container, from Docker's own event stream,
    /// whoever caused it: GhostDock, the CLI, a restart policy, a crash.
    ContainerChanged {
        #[serde(flatten)]
        change: ContainerChange,
    },
    /// The latest figures, every 5 s, to sockets that asked to watch them.
    /// Boxed: far larger than the other events, which would otherwise all
    /// be as large as it. The JSON is the same.
    Metrics { now: Box<crate::metrics::Now> },
}

/// One container event, reduced to what a board needs to decide whether to
/// look again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerChange {
    pub container_id: String,
    /// The container's name, which history is keyed by.
    #[serde(default)]
    pub name: Option<String>,
    /// The compose project, when the container belongs to one.
    pub project: Option<String>,
    /// Docker's verb: `start`, `die`, `health_status: unhealthy`, and so on.
    pub action: String,
}

impl ServerEvent {
    /// Every name [`ServerEvent::name`] can return.
    ///
    /// A browser's `EventSource` delivers named events only to listeners
    /// registered for that name -- `onmessage` sees unnamed events alone --
    /// so a client has to know the full set. Add a variant, add it here.
    pub const NAMES: &'static [&'static str] = &[
        "deployment_started",
        "deployment_output",
        "deployment_finished",
        "container_changed",
        "metrics",
    ];

    /// SSE event name, so a client can listen selectively.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::DeploymentStarted { .. } => "deployment_started",
            Self::DeploymentOutput { .. } => "deployment_output",
            Self::DeploymentFinished { .. } => "deployment_finished",
            Self::ContainerChanged { .. } => "container_changed",
            Self::Metrics { .. } => "metrics",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::{DeploymentStatus, Trigger};

    #[test]
    fn every_event_name_is_listed() {
        // A name missing from NAMES is an event the browser silently never
        // delivers, which is invisible until a feature quietly stops working.
        for event in [
            ServerEvent::DeploymentStarted {
                stack_id: 1,
                deployment_id: 1,
                action: Action::Deploy,
            },
            ServerEvent::DeploymentOutput {
                deployment_id: 1,
                line: String::new(),
            },
            ServerEvent::DeploymentFinished {
                deployment: sample_deployment(),
            },
            ServerEvent::ContainerChanged {
                change: ContainerChange {
                    container_id: "abc".to_owned(),
                    name: None,
                    project: None,
                    action: "start".to_owned(),
                },
            },
            ServerEvent::Metrics {
                now: Box::default(),
            },
        ] {
            assert!(
                ServerEvent::NAMES.contains(&event.name()),
                "{:?} is not in NAMES, so no listener is registered for it",
                event.name()
            );
        }
    }

    fn sample_deployment() -> Deployment {
        Deployment {
            id: 1,
            stack_id: 7,
            action: Action::Deploy,
            trigger: Trigger::Manual,
            status: DeploymentStatus::Succeeded,
            exit_code: Some(0),
            commit_sha: None,
            started_at: chrono::Utc::now(),
            finished_at: None,
        }
    }

    #[test]
    fn a_finished_event_has_no_duplicate_keys() {
        let event = ServerEvent::DeploymentFinished {
            deployment: sample_deployment(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json.matches("\"stack_id\"").count(),
            1,
            "a repeated key is resolved differently by different parsers: {json}"
        );
        assert!(json.contains("\"stack_id\":7"));
    }
}
