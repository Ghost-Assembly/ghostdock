//! Deciding what counts as an update, and saying so in words.

use domain::update::{reason, short_image};
use shared::update::{ImageStatus, UpdateStatus};

fn image(name: &str, running: Option<&str>, available: Option<&str>) -> ImageStatus {
    ImageStatus {
        image: name.to_owned(),
        running: running.map(str::to_owned),
        available: available.map(str::to_owned),
        error: None,
    }
}

#[test]
fn an_unknown_digest_is_never_an_update() {
    // A failed check must not look like a change. People stop believing a
    // list that cries wolf, and then stop reading the ones that matter.
    for (running, available) in [
        (None, Some("sha256:b")),
        (Some("sha256:a"), None),
        (None, None),
    ] {
        assert!(
            !image("nginx", running, available).has_moved(),
            "running={running:?} available={available:?} must not count"
        );
    }
}

#[test]
fn a_changed_digest_is_an_update() {
    assert!(image("nginx", Some("sha256:a"), Some("sha256:b")).has_moved());
    assert!(!image("nginx", Some("sha256:a"), Some("sha256:a")).has_moved());
}

#[test]
fn a_stack_that_has_never_deployed_is_not_behind() {
    // It has nothing to be behind. That is a state to show, not an update.
    let status = UpdateStatus {
        remote_commit: Some("abc".to_owned()),
        deployed_commit: None,
        ..Default::default()
    };
    assert!(!status.is_behind_git());
    assert!(!status.has_update());
}

#[test]
fn a_stack_is_behind_when_the_remote_moved_past_what_was_deployed() {
    let status = UpdateStatus {
        remote_commit: Some("def".to_owned()),
        deployed_commit: Some("abc".to_owned()),
        ..Default::default()
    };
    assert!(status.is_behind_git());
    assert_eq!(reason(&status).as_deref(), Some("a new commit"));
}

#[test]
fn nothing_waiting_has_no_reason() {
    assert_eq!(reason(&UpdateStatus::default()), None);

    let current = UpdateStatus {
        remote_commit: Some("abc".to_owned()),
        deployed_commit: Some("abc".to_owned()),
        images: vec![image("nginx", Some("sha256:a"), Some("sha256:a"))],
        ..Default::default()
    };
    assert_eq!(reason(&current), None);
}

#[test]
fn one_moved_image_is_named() {
    // Naming it is the point: it says what you are about to change.
    let status = UpdateStatus {
        images: vec![
            image("nginx:alpine", Some("sha256:a"), Some("sha256:b")),
            image("redis:7", Some("sha256:c"), Some("sha256:c")),
        ],
        ..Default::default()
    };
    assert_eq!(reason(&status).as_deref(), Some("nginx:alpine moved"));
}

#[test]
fn several_moved_images_are_counted_rather_than_listed() {
    let status = UpdateStatus {
        images: vec![
            image("nginx:alpine", Some("sha256:a"), Some("sha256:b")),
            image("redis:7", Some("sha256:c"), Some("sha256:d")),
        ],
        ..Default::default()
    };
    assert_eq!(reason(&status).as_deref(), Some("2 images moved"));
}

#[test]
fn a_commit_and_an_image_are_reported_together() {
    let status = UpdateStatus {
        remote_commit: Some("def".to_owned()),
        deployed_commit: Some("abc".to_owned()),
        images: vec![image("nginx:alpine", Some("sha256:a"), Some("sha256:b"))],
        ..Default::default()
    };
    assert_eq!(
        reason(&status).as_deref(),
        Some("a new commit and nginx:alpine moved")
    );

    let many = UpdateStatus {
        images: vec![
            image("nginx:alpine", Some("sha256:a"), Some("sha256:b")),
            image("redis:7", Some("sha256:c"), Some("sha256:d")),
        ],
        ..status
    };
    assert_eq!(
        reason(&many).as_deref(),
        Some("a new commit and 2 images moved")
    );
}

#[test]
fn an_image_name_is_trimmed_to_the_part_that_differs() {
    // Registry and namespace are the same for every image in a stack and
    // crowd out the bit a person is looking for.
    assert_eq!(short_image("ghcr.io/team/service/app:1.2"), "app:1.2");
    assert_eq!(short_image("nginx:alpine"), "nginx:alpine");
    assert_eq!(short_image("nginx"), "nginx");
    assert_eq!(short_image("registry.example:5000/team/app"), "app");
    assert_eq!(short_image("registry.example:5000/team/app:v2"), "app:v2");
}
