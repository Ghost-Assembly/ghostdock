//! Integration tests against a real Docker daemon.
//!
//! Deliberately not mocked. Fidelity to the real `docker compose` is the
//! entire argument for shelling out, so a mock would only assert our own
//! assumptions back at us. These skip loudly when no daemon is reachable.

use std::time::Duration;

use compose::command::{Project, Pull};
use compose::{COMPOSE_FILE, Compose, command};

const IMAGE: &str = "alpine:3.22";
const TIMEOUT: Duration = Duration::from_secs(120);

fn daemon_available() -> bool {
    std::process::Command::new(
        std::env::var("GHOSTDOCK_DOCKER_BIN")
            .as_deref()
            .unwrap_or("docker"),
    )
    .args(["info", "--format", "{{.ServerVersion}}"])
    .output()
    .is_ok_and(|o| o.status.success())
}

/// Returns false and prints a notice when there is no daemon, so a skipped
/// test is visible rather than silently passing.
macro_rules! require_daemon {
    () => {
        if !daemon_available() {
            eprintln!("SKIPPED: no Docker daemon reachable");
            return;
        }
    };
}

struct Fixture {
    compose: Compose,
    stack: String,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new(stack: &str, yaml: &str) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let compose = Compose::new(dir.path());
        compose
            .materialise(stack, yaml, &[])
            .await
            .expect("materialise");
        Self {
            compose,
            stack: stack.to_owned(),
            _dir: dir,
        }
    }

    fn project(&self) -> Project<'_> {
        Project {
            name: &self.stack,
            dir: Box::leak(self.compose.project_dir(&self.stack).into_boxed_path()),
            file: COMPOSE_FILE,
            env_file: None,
        }
    }

    async fn run(&self, argv: Vec<String>) -> compose::Outcome {
        self.compose.run(&argv, TIMEOUT, None).await.expect("run")
    }

    async fn teardown(&self) {
        let _ = self.run(command::down(&self.project())).await;
    }
}

fn healthy_stack() -> String {
    format!(
        "services:\n  \
           ok:\n    \
             image: {IMAGE}\n    \
             command: [\"sh\", \"-c\", \"while :; do sleep 3600; done\"]\n    \
             healthcheck:\n      \
               test: [\"CMD\", \"true\"]\n      \
               interval: 1s\n      \
               timeout: 2s\n      \
               retries: 2\n"
    )
}

/// Starts fine, then fails its healthcheck: the "it came up broken" case.
fn broken_stack() -> String {
    format!(
        "services:\n  \
           broken:\n    \
             image: {IMAGE}\n    \
             command: [\"sh\", \"-c\", \"while :; do sleep 3600; done\"]\n    \
             healthcheck:\n      \
               test: [\"CMD\", \"false\"]\n      \
               interval: 1s\n      \
               timeout: 1s\n      \
               retries: 1\n"
    )
}

#[tokio::test]
async fn config_resolves_a_valid_file() {
    require_daemon!();
    let f = Fixture::new("ghostdocktest-config", &healthy_stack()).await;

    let outcome = f.run(command::config(&f.project())).await;
    assert!(outcome.success, "config failed: {}", outcome.output);

    let parsed: serde_json::Value = serde_json::from_str(&outcome.output).expect("json");
    assert!(
        parsed["services"]["ok"].is_object(),
        "compose's own resolved view should list the service: {}",
        outcome.output
    );
}

#[tokio::test]
async fn config_surfaces_the_real_error_for_an_invalid_file() {
    require_daemon!();
    let f = Fixture::new(
        "ghostdocktest-invalid",
        "services:\n  bad:\n    ports: 12\n",
    )
    .await;

    let outcome = f.run(command::config(&f.project())).await;
    assert!(!outcome.success, "an invalid file must not validate");
    assert!(
        !outcome.output.trim().is_empty(),
        "the user needs compose's own message, not a generic failure"
    );
}

#[tokio::test]
async fn up_wait_reports_success_for_a_healthy_stack() {
    require_daemon!();
    let f = Fixture::new("ghostdocktest-healthy", &healthy_stack()).await;

    let outcome = f.run(command::up(&f.project(), Pull::Missing, 60)).await;
    let success = outcome.success;
    let output = outcome.output.clone();
    f.teardown().await;

    assert!(success, "healthy stack should deploy cleanly: {output}");
}

#[tokio::test]
async fn up_wait_reports_failure_when_a_stack_comes_up_broken() {
    require_daemon!();
    // THE claim the architecture rests on. Without `--wait`, compose exits
    // zero the moment containers start, and a stack that is up but failing
    // its healthcheck is reported to the user as a successful deploy.
    let f = Fixture::new("ghostdocktest-broken", &broken_stack()).await;

    let outcome = f.run(command::up(&f.project(), Pull::Missing, 25)).await;
    let success = outcome.success;
    let output = outcome.output.clone();
    f.teardown().await;

    assert!(
        !success,
        "a stack that starts but fails its healthcheck must be a FAILED \
         deploy, not a successful one. Output:\n{output}"
    );
}
