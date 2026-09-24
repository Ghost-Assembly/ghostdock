//! Materialising a stack's project directory.

use std::os::unix::fs::PermissionsExt;

use compose::{COMPOSE_FILE, Compose, ENV_FILE};

fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[tokio::test]
async fn writes_the_compose_file_into_a_directory_named_for_the_stack() {
    let root = tempfile::tempdir().unwrap();
    let compose = Compose::new(root.path());

    let dir = compose
        .materialise("blog", "services: {}\n", &[])
        .await
        .unwrap();

    assert_eq!(dir, root.path().join("blog"));
    let written = tokio::fs::read_to_string(dir.join(COMPOSE_FILE))
        .await
        .unwrap();
    assert_eq!(written, "services: {}\n");
}

#[tokio::test]
async fn the_env_file_is_readable_only_by_its_owner() {
    let root = tempfile::tempdir().unwrap();
    let compose = Compose::new(root.path());

    let dir = compose
        .materialise("secrets", "services: {}\n", &vars(&[("TOKEN", "hunter2")]))
        .await
        .unwrap();

    let mode = tokio::fs::metadata(dir.join(ENV_FILE))
        .await
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o600,
        "a stack's .env holds its secrets and must not be world-readable"
    );
}

#[tokio::test]
async fn removing_every_variable_removes_the_env_file() {
    let root = tempfile::tempdir().unwrap();
    let compose = Compose::new(root.path());

    compose
        .materialise("app", "services: {}\n", &vars(&[("GONE", "1")]))
        .await
        .unwrap();
    let dir = compose
        .materialise("app", "services: {}\n", &[])
        .await
        .unwrap();

    assert!(
        !dir.join(ENV_FILE).exists(),
        "a stale .env would keep applying variables the user deleted"
    );
}

#[tokio::test]
async fn a_traversing_name_is_refused_before_anything_is_written() {
    let root = tempfile::tempdir().unwrap();
    let compose = Compose::new(root.path());

    assert!(
        compose
            .materialise("../escape", "services: {}\n", &[])
            .await
            .is_err()
    );
    assert!(
        !root.path().parent().unwrap().join("escape").exists(),
        "nothing may be created outside the stacks root"
    );
}
