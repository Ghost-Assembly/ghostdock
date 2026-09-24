//! The daemon's event stream, reduced to container changes.

use bollard::models::{EventMessage, EventMessageTypeEnum};
use shared::event::ContainerChange;

use crate::map::LABEL_PROJECT;

/// Actions that change what a board shows. Health events arrive as
/// `health_status: <verdict>` and are matched by prefix.
const STATE_CHANGES: &[&str] = &[
    "create", "start", "restart", "die", "stop", "kill", "oom", "pause", "unpause", "destroy",
    "rename",
];
const HEALTH: &str = "health_status";

fn matters(action: &str) -> bool {
    STATE_CHANGES.contains(&action) || action.starts_with(HEALTH)
}

/// The change an event describes, if it is one a board should react to.
#[must_use]
pub fn to_change(event: EventMessage) -> Option<ContainerChange> {
    if event.typ != Some(EventMessageTypeEnum::CONTAINER) {
        return None;
    }
    let action = event.action?;
    if !matters(&action) {
        return None;
    }
    let actor = event.actor?;
    let container_id = actor.id.filter(|id| !id.is_empty())?;
    let attributes = actor.attributes.unwrap_or_default();
    let project = attributes.get(LABEL_PROJECT).cloned();
    let name = attributes.get("name").cloned();
    Some(ContainerChange {
        container_id,
        name,
        project,
        action,
    })
}

/// The daemon-side filter: containers only, and only the actions above, so
/// chatter never crosses the socket. [`to_change`] checks again, which keeps
/// correctness independent of how a daemon interprets the filter.
pub(crate) fn filters() -> std::collections::HashMap<String, Vec<String>> {
    let mut events: Vec<String> = STATE_CHANGES.iter().map(|&a| a.to_owned()).collect();
    events.push(HEALTH.to_owned());
    std::collections::HashMap::from([
        ("type".to_owned(), vec!["container".to_owned()]),
        ("event".to_owned(), events),
    ])
}
