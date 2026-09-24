//! Working out what is safe to remove.
//!
//! Pure, so the rule can be tested without a daemon. The rule matters: this
//! deletes things, and an image removed by mistake is only recoverable by
//! downloading it again.

use std::collections::HashSet;

use bollard::models::{ContainerSummary, ContainerSummaryStateEnum, ImageSummary};
use shared::cleanup::{CleanupPreview, StoppedContainer, UnusedImage};

use crate::map::LABEL_PROJECT;

/// Images no container refers to, split by whether they still have a name.
///
/// "Referred to" counts stopped containers as well as running ones. A
/// stopped container is something a person intends to start again, and
/// removing the image underneath it would turn that into a download.
#[must_use]
pub fn preview(images: &[ImageSummary], containers: &[ContainerSummary]) -> CleanupPreview {
    let in_use: HashSet<&str> = containers
        .iter()
        .filter_map(|c| c.image_id.as_deref())
        .collect();

    let mut dangling = Vec::new();
    let mut unused = Vec::new();

    for image in images {
        if in_use.contains(image.id.as_str()) {
            continue;
        }

        let tags: Vec<String> = image
            .repo_tags
            .iter()
            // The daemon writes this placeholder for an image whose name was
            // taken by a newer build; it is not a name anyone can use.
            .filter(|tag| *tag != "<none>:<none>")
            .cloned()
            .collect();

        let entry = UnusedImage {
            id: image.id.clone(),
            dangling: tags.is_empty(),
            size_bytes: u64::try_from(image.size).unwrap_or(0),
            tags,
        };

        if entry.dangling {
            dangling.push(entry);
        } else {
            unused.push(entry);
        }
    }

    // Largest first: the screen's job is to show where the space went.
    dangling.sort_by_key(|image| std::cmp::Reverse(image.size_bytes));
    unused.sort_by_key(|image| std::cmp::Reverse(image.size_bytes));

    CleanupPreview {
        dangling,
        unused,
        ..CleanupPreview::default()
    }
}

/// Stopped containers nothing appears to need, as (left by compose projects
/// that are gone, standalone).
///
/// Offered: not running, and either outside Compose or from a project that
/// is neither registered here nor has anything of its own still running. A
/// registered stack that was stopped keeps its containers, and an exited
/// one-shot job beside a live service is part of that service's stack.
#[must_use]
pub fn stopped_containers(
    containers: &[ContainerSummary],
    registered: &HashSet<String>,
) -> (Vec<StoppedContainer>, Vec<StoppedContainer>) {
    let project_of = |c: &ContainerSummary| {
        c.labels
            .as_ref()
            .and_then(|labels| labels.get(LABEL_PROJECT))
            .cloned()
    };
    let stopped = |c: &ContainerSummary| {
        matches!(
            c.state,
            Some(
                ContainerSummaryStateEnum::EXITED
                    | ContainerSummaryStateEnum::CREATED
                    | ContainerSummaryStateEnum::DEAD
            )
        )
    };
    let alive: HashSet<String> = containers
        .iter()
        .filter(|c| !stopped(c))
        .filter_map(project_of)
        .collect();

    let mut leftover = Vec::new();
    let mut standalone = Vec::new();
    for c in containers.iter().filter(|c| stopped(c)) {
        let Some(id) = c.id.clone() else { continue };
        let project = project_of(c);
        let entry = StoppedContainer {
            name: crate::map::primary_name(c.names.as_deref(), &id),
            id,
            image: c.image.clone().unwrap_or_default(),
            project: project.clone(),
            status: c.status.clone().unwrap_or_default(),
        };
        match project {
            None => standalone.push(entry),
            Some(p) if !registered.contains(&p) && !alive.contains(&p) => leftover.push(entry),
            Some(_) => {}
        }
    }
    leftover.sort_by(|a, b| a.name.cmp(&b.name));
    standalone.sort_by(|a, b| a.name.cmp(&b.name));
    (leftover, standalone)
}
