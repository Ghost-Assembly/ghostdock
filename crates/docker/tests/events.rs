//! Reducing daemon events to the changes a board cares about.

use bollard::models::{EventActor, EventMessage, EventMessageTypeEnum};
use docker::events::to_change;
use std::collections::HashMap;

fn event(typ: EventMessageTypeEnum, action: &str, project: Option<&str>) -> EventMessage {
    let mut attributes = HashMap::new();
    attributes.insert("name".to_owned(), "blog-web-1".to_owned());
    if let Some(p) = project {
        attributes.insert("com.docker.compose.project".to_owned(), p.to_owned());
    }
    EventMessage {
        typ: Some(typ),
        action: Some(action.to_owned()),
        actor: Some(EventActor {
            id: Some("abc123".to_owned()),
            attributes: Some(attributes),
        }),
        ..Default::default()
    }
}

#[test]
fn a_container_starting_is_a_change_to_its_project() {
    let change = to_change(event(
        EventMessageTypeEnum::CONTAINER,
        "start",
        Some("blog"),
    ))
    .unwrap();
    assert_eq!(change.container_id, "abc123");
    assert_eq!(change.project.as_deref(), Some("blog"));
    assert_eq!(change.action, "start");
    assert_eq!(change.name.as_deref(), Some("blog-web-1"));
}

#[test]
fn a_container_outside_compose_still_counts() {
    let change = to_change(event(EventMessageTypeEnum::CONTAINER, "die", None)).unwrap();
    assert_eq!(change.project, None);
}

#[test]
fn health_changes_count_with_their_verdict() {
    let change = to_change(event(
        EventMessageTypeEnum::CONTAINER,
        "health_status: unhealthy",
        Some("blog"),
    ))
    .unwrap();
    assert_eq!(change.action, "health_status: unhealthy");
}

#[test]
fn every_state_change_counts() {
    for action in [
        "create", "start", "restart", "die", "stop", "kill", "oom", "pause", "unpause", "destroy",
        "rename",
    ] {
        assert!(
            to_change(event(EventMessageTypeEnum::CONTAINER, action, None)).is_some(),
            "{action}"
        );
    }
}

#[test]
fn chatter_that_changes_nothing_is_dropped() {
    // A shell or a log reader produces a stream of these; refreshing the
    // board on each would turn watching a container into load on it.
    for action in [
        "exec_create: sh",
        "exec_start: sh",
        "exec_die",
        "attach",
        "resize",
        "top",
        "archive-path",
    ] {
        assert!(
            to_change(event(EventMessageTypeEnum::CONTAINER, action, None)).is_none(),
            "{action}"
        );
    }
}

#[test]
fn events_about_other_objects_are_dropped() {
    assert!(to_change(event(EventMessageTypeEnum::IMAGE, "pull", None)).is_none());
    assert!(to_change(event(EventMessageTypeEnum::NETWORK, "connect", None)).is_none());
}

#[test]
fn an_event_without_a_container_id_is_dropped() {
    let mut e = event(EventMessageTypeEnum::CONTAINER, "start", None);
    e.actor = None;
    assert!(to_change(e).is_none());
}
