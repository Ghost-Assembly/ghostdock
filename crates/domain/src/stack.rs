//! Grouping containers into stacks.
//!
//! The stack is the primary unit of the UI, so this is where a flat list of
//! containers becomes the thing a user actually thinks about.

use std::collections::BTreeMap;

use shared::container::{Container, Health};
use shared::stack::{Managed, Stack, StackState};

/// The result of grouping a host's containers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grouped {
    /// Compose-managed containers, grouped by project and sorted by name.
    pub stacks: Vec<Stack>,
    /// Containers started outside Compose, sorted by name.
    pub unmanaged: Vec<Container>,
}

/// Groups containers by Compose project.
///
/// Never yields [`StackState::Empty`]: a group exists here only because a
/// container exists. That variant is for stacks registered in the database
/// that currently have nothing running.
#[must_use]
pub fn group(containers: Vec<Container>) -> Grouped {
    let mut by_project: BTreeMap<String, Vec<Container>> = BTreeMap::new();
    let mut unmanaged = Vec::new();

    for container in containers {
        match &container.compose {
            Some(membership) => by_project
                .entry(membership.project.clone())
                .or_default()
                .push(container),
            None => unmanaged.push(container),
        }
    }

    // BTreeMap already orders projects by name.
    let stacks = by_project
        .into_iter()
        .map(|(project, mut containers)| {
            containers.sort_by(|a, b| a.name.cmp(&b.name));
            let total_count = containers.len();
            let running_count = containers.iter().filter(|c| c.state.is_running()).count();
            Stack {
                state: state_of(&containers, running_count, total_count),
                project,
                containers,
                running_count,
                total_count,
                managed: None,
                icon: None,
            }
        })
        .collect();

    unmanaged.sort_by(|a, b| a.name.cmp(&b.name));

    Grouped { stacks, unmanaged }
}

/// Aggregate state of one stack's containers.
///
/// Order matters. "Nothing is running" is decided before health, because a
/// container that has exited keeps reporting its last health result — Docker
/// reports `health=unhealthy` on a stopped container whose healthcheck can no
/// longer pass. Reading that as live would show a deliberately stopped stack
/// as an alarm. Among containers that *are* running, unhealthy outranks
/// everything: a stack that is fully up but failing its healthcheck is not
/// something to paint green.
fn state_of(containers: &[Container], running_count: usize, total_count: usize) -> StackState {
    if total_count == 0 {
        return StackState::Empty;
    }
    if running_count == 0 {
        return StackState::Stopped;
    }
    if containers
        .iter()
        .filter(|c| c.state.is_running())
        .any(|c| c.health == Some(Health::Unhealthy))
    {
        return StackState::Unhealthy;
    }
    if running_count == total_count {
        StackState::Running
    } else {
        StackState::Degraded
    }
}

/// Merges observed containers with the stacks GhostDock manages.
///
/// A managed stack with nothing running still appears, because "I deployed
/// this and it is not running" is exactly what a user needs to see. A project
/// running on the host that GhostDock does not manage also appears, so the list
/// reflects the machine rather than only the database.
#[must_use]
pub fn merge(containers: Vec<Container>, managed: Vec<(String, Managed)>) -> Grouped {
    let mut grouped = group(containers);
    let mut by_project: BTreeMap<String, Managed> = managed.into_iter().collect();

    for stack in &mut grouped.stacks {
        stack.managed = by_project.remove(&stack.project);
    }

    // Whatever is left is managed but has no containers on the host.
    for (project, managed) in by_project {
        grouped.stacks.push(Stack {
            project,
            state: StackState::Empty,
            containers: Vec::new(),
            running_count: 0,
            total_count: 0,
            managed: Some(managed),
            icon: None,
        });
    }

    grouped.stacks.sort_by(|a, b| a.project.cmp(&b.project));
    grouped
}
