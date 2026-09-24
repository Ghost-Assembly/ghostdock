//! Stack wire types.
//!
//! A stack is the primary unit of the UI. Most stacks are a single service,
//! so a container-centric view shows a user far more rows than they have
//! things to think about.

use serde::{Deserialize, Serialize};

use crate::container::Container;
use crate::deployment::SourceKind;

/// Present when GhostDock holds this stack's definition, as opposed to merely
/// observing containers Compose put on the host.
///
/// Keeping both in one list matters: a user thinks in terms of "my stacks",
/// not "stacks GhostDock knows about" versus "stacks that exist".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Managed {
    pub id: i64,
    pub name: String,
    pub source_kind: SourceKind,
    /// True while an operation is in flight, so the UI can say so.
    pub busy: bool,
}

/// Aggregate state of a stack's containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackState {
    /// Every container is running.
    Running,
    /// Some but not all containers are running.
    Degraded,
    /// No container is running.
    Stopped,
    /// At least one container reports unhealthy.
    Unhealthy,
    /// The stack has no containers at all.
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stack {
    /// Compose project name.
    pub project: String,
    pub state: StackState,
    pub containers: Vec<Container>,
    pub running_count: usize,
    pub total_count: usize,
    /// `None` for a stack running on the host that GhostDock does not manage.
    pub managed: Option<Managed>,
}
