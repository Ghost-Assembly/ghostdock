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

#[tokio::test]
async fn forgetting_a_stack_removes_its_env_file_and_nothing_else() {
    // The .env holds the stack's secrets in plain text; the rest of the
    // directory may hold data the stack's containers still use.
    let root = tempfile::tempdir().unwrap();
    let compose = Compose::new(root.path());
    let dir = compose
        .materialise("app", "services: {}\n", &vars(&[("TOKEN", "hunter2")]))
        .await
        .unwrap();
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("data/keep.db"), "keep").unwrap();

    compose.remove_env_file("app").await.unwrap();

    assert!(!dir.join(ENV_FILE).exists());
    assert!(dir.join(COMPOSE_FILE).exists());
    assert!(dir.join("data/keep.db").exists());

    // Nothing to remove is not a failure, and a bad name is refused.
    compose.remove_env_file("app").await.unwrap();
    compose.remove_env_file("never-deployed").await.unwrap();
    assert!(compose.remove_env_file("../escape").await.is_err());
}

#[tokio::test]
async fn a_compose_file_that_compose_would_find_on_its_own_is_reported() {
    // Given no file, compose looks in the project directory and then in
    // every directory above it, and would act on whatever it found there.
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("stacks/app");
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(compose::default_file_above(&dir).await, None);

    for name in [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yaml",
        "docker-compose.yml",
    ] {
        let stray = root.path().join(name);
        std::fs::write(&stray, "services: {}\n").unwrap();
        assert_eq!(
            compose::default_file_above(&dir).await,
            Some(stray.clone()),
            "{name}"
        );
        std::fs::remove_file(&stray).unwrap();
    }
}
