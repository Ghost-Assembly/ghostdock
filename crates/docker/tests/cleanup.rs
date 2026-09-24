//! Deciding what is safe to remove.
//!
//! This deletes things, so the rule gets tested rather than trusted.

use bollard::models::{ContainerSummary, ImageSummary};
use docker::cleanup::preview;

fn image(id: &str, tags: &[&str], size: i64) -> ImageSummary {
    ImageSummary {
        id: id.to_owned(),
        repo_tags: tags.iter().map(|t| (*t).to_owned()).collect(),
        size,
        ..Default::default()
    }
}

fn container(image_id: &str) -> ContainerSummary {
    ContainerSummary {
        image_id: Some(image_id.to_owned()),
        ..Default::default()
    }
}

#[test]
fn an_image_a_container_uses_is_left_alone() {
    let result = preview(
        &[image("sha256:a", &["nginx:alpine"], 100)],
        &[container("sha256:a")],
    );
    assert!(result.is_empty());
}

#[test]
fn a_stopped_containers_image_is_still_in_use() {
    // A stopped container is something someone means to start again.
    // Removing the image under it turns that into a download.
    let result = preview(
        &[image("sha256:a", &["nginx:alpine"], 100)],
        // The list includes stopped containers; nothing here says running.
        &[container("sha256:a")],
    );
    assert!(
        result.is_empty(),
        "a stopped container still protects its image"
    );
}

#[test]
fn an_untagged_image_is_dangling() {
    let result = preview(&[image("sha256:abcdef123456789", &[], 500)], &[]);

    assert_eq!(result.dangling.len(), 1);
    assert!(result.unused.is_empty());
    assert_eq!(result.dangling_bytes(), 500);
    assert!(
        result.dangling[0]
            .label()
            .starts_with("untagged abcdef1234")
    );
}

#[test]
fn the_none_placeholder_is_not_treated_as_a_name() {
    // The daemon writes <none>:<none> for an image whose name a newer build
    // took. It is not a name anyone can use, so such an image is dangling.
    let result = preview(&[image("sha256:a", &["<none>:<none>"], 10)], &[]);
    assert_eq!(result.dangling.len(), 1);
    assert!(result.unused.is_empty());
}

#[test]
fn a_named_image_nothing_uses_is_unused_not_dangling() {
    let result = preview(&[image("sha256:a", &["redis:7"], 300)], &[]);

    assert!(result.dangling.is_empty());
    assert_eq!(result.unused.len(), 1);
    assert_eq!(result.unused[0].label(), "redis:7");
    assert_eq!(result.unused_bytes(), 300);
}

#[test]
fn the_largest_are_listed_first() {
    // The screen's job is to show where the space went.
    let result = preview(
        &[
            image("sha256:small", &["a:1"], 10),
            image("sha256:big", &["b:1"], 900),
            image("sha256:mid", &["c:1"], 400),
        ],
        &[],
    );

    let sizes: Vec<u64> = result.unused.iter().map(|i| i.size_bytes).collect();
    assert_eq!(sizes, [900, 400, 10]);
}

#[test]
fn used_and_unused_are_separated_correctly_together() {
    let result = preview(
        &[
            image("sha256:used", &["nginx:alpine"], 100),
            image("sha256:old", &[], 50),
            image("sha256:spare", &["redis:7"], 300),
        ],
        &[container("sha256:used")],
    );

    assert_eq!(result.dangling.len(), 1);
    assert_eq!(result.unused.len(), 1);
    assert_eq!(result.unused[0].label(), "redis:7");
    assert_eq!(result.dangling_bytes() + result.unused_bytes(), 350);
}

// ---- containers -----------------------------------------------------------

use bollard::models::ContainerSummaryStateEnum as State;
use docker::cleanup::stopped_containers;
use std::collections::{HashMap, HashSet};

fn ctr(name: &str, state: State, project: Option<&str>) -> ContainerSummary {
    let mut labels = HashMap::new();
    if let Some(p) = project {
        labels.insert("com.docker.compose.project".to_owned(), p.to_owned());
    }
    ContainerSummary {
        id: Some(format!("id-{name}")),
        names: Some(vec![format!("/{name}")]),
        image: Some("alpine:3.22".to_owned()),
        state: Some(state),
        status: Some("Exited (0) 3 weeks ago".to_owned()),
        labels: Some(labels),
        ..Default::default()
    }
}

fn names(list: &[shared::cleanup::StoppedContainer]) -> Vec<&str> {
    list.iter().map(|c| c.name.as_str()).collect()
}

fn none() -> HashSet<String> {
    HashSet::new()
}

#[test]
fn a_running_container_is_never_offered() {
    for state in [State::RUNNING, State::PAUSED, State::RESTARTING] {
        let (left, alone) = stopped_containers(
            &[ctr("a", state, Some("gone")), ctr("b", state, None)],
            &none(),
        );
        assert!(left.is_empty() && alone.is_empty(), "{state:?}");
    }
}

#[test]
fn a_stopped_container_of_a_project_nothing_runs_is_left_behind() {
    let (left, alone) = stopped_containers(
        &[
            ctr("old-web-1", State::EXITED, Some("old")),
            ctr("old-db-1", State::DEAD, Some("old")),
            ctr("old-init-1", State::CREATED, Some("old")),
        ],
        &none(),
    );
    assert_eq!(names(&left), ["old-db-1", "old-init-1", "old-web-1"]);
    assert!(alone.is_empty());
    assert_eq!(left[0].project.as_deref(), Some("old"));
}

#[test]
fn a_registered_stack_that_was_stopped_is_left_alone() {
    // Stopping is a decision. Its containers are how it starts again.
    let registered = HashSet::from(["kept".to_owned()]);
    let (left, _) = stopped_containers(
        &[ctr("kept-web-1", State::EXITED, Some("kept"))],
        &registered,
    );
    assert!(left.is_empty());
}

#[test]
fn an_exited_container_beside_a_running_one_belongs_to_it() {
    // A one-shot migration or init job that exited 0 is part of a live
    // stack, not debris.
    let (left, _) = stopped_containers(
        &[
            ctr("app-web-1", State::RUNNING, Some("app")),
            ctr("app-migrate-1", State::EXITED, Some("app")),
        ],
        &none(),
    );
    assert!(left.is_empty());
}

#[test]
fn a_stopped_container_outside_compose_is_offered_separately() {
    let (left, alone) = stopped_containers(&[ctr("scratch", State::EXITED, None)], &none());
    assert!(left.is_empty());
    assert_eq!(names(&alone), ["scratch"]);
    assert_eq!(alone[0].status, "Exited (0) 3 weeks ago");
}
