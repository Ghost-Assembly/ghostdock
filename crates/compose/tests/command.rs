//! Compose invocations.
//!
//! These pin the flags that carry real consequences: project identity,
//! deploy-success semantics, and never destroying data.

use compose::command::{Project, Pull, config, down, restart, stop, up};
use std::path::Path;

fn project() -> Project<'static> {
    Project {
        name: "blog",
        dir: Path::new("/var/lib/ghostdock/stacks/blog"),
        file: "docker-compose.yml",
        env_file: None,
    }
}

fn pairs(argv: &[String], flag: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == flag)
        .map(|i| argv[i + 1].clone())
}

#[test]
fn every_invocation_names_its_project_and_directory() {
    for argv in [
        config(&project()),
        up(&project(), Pull::Always, 120),
        down(&project()),
        stop(&project()),
        restart(&project()),
    ] {
        assert_eq!(argv[0], "compose");
        assert_eq!(
            pairs(&argv, "--project-name").as_deref(),
            Some("blog"),
            "without an explicit project name, compose infers one from the \
             directory and can adopt or orphan the wrong containers"
        );
        assert_eq!(
            pairs(&argv, "--project-directory").as_deref(),
            Some("/var/lib/ghostdock/stacks/blog")
        );
        assert_eq!(
            pairs(&argv, "--file").as_deref(),
            Some("/var/lib/ghostdock/stacks/blog/docker-compose.yml")
        );
    }
}

#[test]
fn an_env_file_is_passed_only_when_present() {
    assert!(!up(&project(), Pull::Always, 120).contains(&"--env-file".to_owned()));

    // Absolute, and free to live outside the compose file's directory.
    let env_path = Path::new("/var/lib/ghostdock/stacks/blog/.env");
    let with_env = Project {
        env_file: Some(env_path),
        ..project()
    };
    assert_eq!(
        pairs(&up(&with_env, Pull::Always, 120), "--env-file").as_deref(),
        Some("/var/lib/ghostdock/stacks/blog/.env")
    );

    // A Git stack runs against the compose file where it sits in the
    // repository, with its env file kept outside the working tree.
    let in_repo = Project {
        name: "blog",
        dir: Path::new("/var/lib/ghostdock/stacks/blog/repo/compose"),
        file: "app.yml",
        env_file: Some(env_path),
    };
    let argv = up(&in_repo, Pull::Always, 120);
    assert_eq!(
        pairs(&argv, "--project-directory").as_deref(),
        Some("/var/lib/ghostdock/stacks/blog/repo/compose"),
        "relative paths in the file must resolve where its author expected"
    );
    assert_eq!(
        pairs(&argv, "--env-file").as_deref(),
        Some("/var/lib/ghostdock/stacks/blog/.env"),
        "the env file must not be written into the repository"
    );
}

#[test]
fn config_asks_for_json() {
    let argv = config(&project());
    assert!(argv.contains(&"config".to_owned()));
    assert_eq!(pairs(&argv, "--format").as_deref(), Some("json"));
}

#[test]
fn up_waits_for_services_to_be_healthy() {
    let argv = up(&project(), Pull::Always, 90);
    assert!(
        argv.contains(&"--wait".to_owned()),
        "without --wait a stack that starts and crash-loops reports success"
    );
    assert_eq!(pairs(&argv, "--wait-timeout").as_deref(), Some("90"));
    assert!(argv.contains(&"--detach".to_owned()));
}

#[test]
fn up_removes_orphans_and_honours_the_pull_policy() {
    let argv = up(&project(), Pull::Always, 120);
    assert!(
        argv.contains(&"--remove-orphans".to_owned()),
        "a service deleted from the file otherwise leaves a container holding \
         its ports and name"
    );
    assert_eq!(pairs(&argv, "--pull").as_deref(), Some("always"));
    assert_eq!(
        pairs(&up(&project(), Pull::Missing, 120), "--pull").as_deref(),
        Some("missing")
    );
    assert_eq!(
        pairs(&up(&project(), Pull::Never, 120), "--pull").as_deref(),
        Some("never")
    );
}

#[test]
fn down_never_removes_volumes() {
    let argv = down(&project());
    assert!(
        !argv.contains(&"--volumes".to_owned()) && !argv.contains(&"-v".to_owned()),
        "stopping a stack must never be a way to destroy its data"
    );
    assert!(argv.contains(&"down".to_owned()));
}

#[test]
fn stop_and_restart_do_not_remove_anything() {
    for argv in [stop(&project()), restart(&project())] {
        assert!(!argv.contains(&"--remove-orphans".to_owned()));
        assert!(!argv.contains(&"--volumes".to_owned()));
    }
}
