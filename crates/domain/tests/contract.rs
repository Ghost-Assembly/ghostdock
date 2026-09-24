//! The path contract.
//!
//! When it is broken the failure is silent -- the deploy succeeds and the
//! application reads an empty directory -- so the check is what turns it
//! into something a person is told about.

use domain::contract::{Mount, PathContract, check};

fn mount(source: &str, destination: &str) -> Mount {
    Mount {
        source: source.to_owned(),
        destination: destination.to_owned(),
    }
}

const SOCKET: (&str, &str) = ("/var/run/docker.sock", "/var/run/docker.sock");

fn with_socket(extra: Mount) -> Vec<Mount> {
    vec![mount(SOCKET.0, SOCKET.1), extra]
}

#[test]
fn the_same_path_on_both_sides_is_satisfied() {
    let mounts = with_socket(mount("/var/lib/ghostdock", "/var/lib/ghostdock"));
    assert_eq!(
        check("/var/lib/ghostdock", Some(&mounts)),
        PathContract::Satisfied
    );
}

#[test]
fn a_different_host_path_is_a_violation() {
    // The mistake the README warns about.
    let mounts = with_socket(mount("/srv/ghostdock", "/var/lib/ghostdock"));
    let result = check("/var/lib/ghostdock", Some(&mounts));

    assert_eq!(
        result,
        PathContract::Violated {
            container_path: "/var/lib/ghostdock".to_owned(),
            host_path: "/srv/ghostdock".to_owned(),
        }
    );
    let message = result.problem().expect("a problem to report");
    assert!(
        message.contains("-v /var/lib/ghostdock:/var/lib/ghostdock"),
        "says how to fix it: {message}"
    );
}

#[test]
fn a_data_directory_inside_a_larger_mount_is_satisfied() {
    // `/srv:/srv` with the data in /srv/ghostdock is correct; an exact-match
    // check would wrongly call it broken.
    let mounts = with_socket(mount("/srv", "/srv"));
    assert_eq!(
        check("/srv/ghostdock", Some(&mounts)),
        PathContract::Satisfied
    );
}

#[test]
fn a_data_directory_inside_a_moved_mount_is_a_violation() {
    let mounts = with_socket(mount("/mnt/storage", "/srv"));
    assert_eq!(
        check("/srv/ghostdock", Some(&mounts)),
        PathContract::Violated {
            container_path: "/srv/ghostdock".to_owned(),
            host_path: "/mnt/storage/ghostdock".to_owned(),
        }
    );
}

#[test]
fn the_most_specific_mount_governs() {
    // An outer mount that is correct does not excuse an inner one that is not.
    let mounts = vec![
        mount("/srv", "/srv"),
        mount("/elsewhere/ghostdock", "/srv/ghostdock"),
    ];
    assert!(matches!(
        check("/srv/ghostdock", Some(&mounts)),
        PathContract::Violated { .. }
    ));
}

#[test]
fn a_path_that_merely_shares_a_prefix_is_not_the_mount() {
    // `/var/lib/ghostdock-old` is not inside `/var/lib/ghostdock`; comparing strings
    // rather than path components would say it was.
    let mounts = with_socket(mount("/var/lib/ghostdock", "/var/lib/ghostdock"));
    assert_eq!(
        check("/var/lib/ghostdock-old", Some(&mounts)),
        PathContract::Unmounted {
            container_path: "/var/lib/ghostdock-old".to_owned()
        }
    );
}

#[test]
fn an_unmounted_data_directory_is_reported() {
    // Lives only in the container: lost on replacement, and relative paths
    // point at nothing on the host.
    let mounts = vec![mount(SOCKET.0, SOCKET.1)];
    let result = check("/var/lib/ghostdock", Some(&mounts));
    assert!(matches!(result, PathContract::Unmounted { .. }));
    assert!(result.problem().unwrap().contains("lost"));
}

#[test]
fn an_anonymous_volume_is_a_violation() {
    // The Dockerfile's VOLUME line makes one if nothing is mounted, and its
    // host path is somewhere under Docker's own storage.
    let mounts = with_socket(mount(
        "/var/lib/docker/volumes/3f9a/_data",
        "/var/lib/ghostdock",
    ));
    assert!(matches!(
        check("/var/lib/ghostdock", Some(&mounts)),
        PathContract::Violated { .. }
    ));
}

#[test]
fn trailing_slashes_and_dots_do_not_matter() {
    let mounts = with_socket(mount("/var/lib/ghostdock/", "/var/lib/./ghostdock"));
    assert_eq!(
        check("/var/lib/ghostdock/", Some(&mounts)),
        PathContract::Satisfied
    );
}

#[test]
fn outside_a_container_there_is_nothing_to_report() {
    let result = check("/var/lib/ghostdock", None);
    assert_eq!(result, PathContract::Unknown);
    assert_eq!(result.problem(), None);
}
