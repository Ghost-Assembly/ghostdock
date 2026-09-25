//! Telling a caller what went wrong without handing it bollard.

use docker::{Error, ErrorKind};

fn answered(status_code: u16, message: &str) -> Error {
    Error::Api(bollard::errors::Error::DockerResponseServerError {
        status_code,
        message: message.to_owned(),
    })
}

#[test]
fn a_missing_object_is_not_found() {
    let e = answered(404, "No such container: nope");
    assert_eq!(e.kind(), ErrorKind::NotFound);
    assert_eq!(e.daemon_message(), Some("No such container: nope"));
}

#[test]
fn a_request_at_odds_with_the_objects_state_is_a_conflict() {
    let e = answered(409, "container abc is not running");
    assert_eq!(e.kind(), ErrorKind::Conflict);
    assert_eq!(e.daemon_message(), Some("container abc is not running"));
}

#[test]
fn anything_else_means_the_daemon_is_not_answering_properly() {
    assert_eq!(answered(500, "server error").kind(), ErrorKind::Unavailable);
    let unreachable = Error::Unreachable(bollard::errors::Error::SocketNotFoundError(
        "/var/run/docker.sock".to_owned(),
    ));
    assert_eq!(unreachable.kind(), ErrorKind::Unavailable);
    assert_eq!(unreachable.daemon_message(), None);
}
