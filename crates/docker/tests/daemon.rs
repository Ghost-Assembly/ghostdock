//! Against a real Docker daemon. Skips loudly when none is reachable.
//!
//! The container here is created by the test and removed by it; nothing
//! else on the host is touched.

use std::process::Command;
use std::time::Duration;

use futures::StreamExt as _;

fn docker(args: &[&str]) -> Option<String> {
    let out = Command::new("docker").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

struct Removed(String);

impl Drop for Removed {
    fn drop(&mut self) {
        let _ = docker(&["rm", "-f", &self.0]);
    }
}

#[tokio::test]
async fn start_health_and_removal_arrive_as_changes() {
    if docker(&["info", "--format", "{{.ServerVersion}}"]).is_none() {
        eprintln!("SKIPPED: no Docker daemon reachable");
        return;
    }
    let client = docker::Client::connect().expect("client");
    // The subscription is made when the stream is first polled, so it is
    // drained from a task that starts before the container does.
    let (tx, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let stream = client.container_changes();
    tokio::spawn(async move {
        let mut stream = Box::pin(stream);
        while let Some(change) = stream.next().await {
            if tx.send(change).is_err() {
                return;
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;

    let name = format!("ghostdock-test-events-{}", std::process::id());
    let _cleanup = Removed(name.clone());
    let id = docker(&[
        "run",
        "-d",
        "--name",
        &name,
        "--health-cmd",
        "true",
        "--health-interval",
        "1s",
        "alpine:3.22",
        "sleep",
        "120",
    ])
    .expect("run a container");

    let mut seen: Vec<String> = Vec::new();
    let waited = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(change) = changes.recv().await {
            let change = change.expect("event");
            if change.container_id != id {
                continue;
            }
            seen.push(change.action.clone());
            if change.action.starts_with("health_status") {
                break;
            }
        }
    })
    .await;
    assert!(waited.is_ok(), "no health event within 30s; saw {seen:?}");
    assert!(seen.iter().any(|a| a == "start"), "{seen:?}");
    assert!(
        seen.iter().any(|a| a == "health_status: healthy"),
        "the daemon filter must pass health verdicts through: {seen:?}"
    );

    docker(&["rm", "-f", &name]).expect("remove");
    let removed = tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(Ok(change)) = changes.recv().await {
            if change.container_id == id && change.action == "destroy" {
                return;
            }
        }
    })
    .await;
    assert!(removed.is_ok(), "removal was not reported");
}

#[tokio::test]
async fn following_logs_yields_lines_as_they_are_written_and_ends_with_the_container() {
    if docker(&["info", "--format", "{{.ServerVersion}}"]).is_none() {
        eprintln!("SKIPPED: no Docker daemon reachable");
        return;
    }
    let client = docker::Client::connect().expect("client");
    let name = format!("ghostdock-test-follow-{}", std::process::id());
    let _cleanup = Removed(name.clone());
    docker(&[
        "run",
        "-d",
        "--name",
        &name,
        "alpine:3.22",
        "sh",
        "-c",
        "i=0; while [ $i -lt 6 ]; do echo line $i; echo oops $i >&2; i=$((i+1)); sleep 0.5; done",
    ])
    .expect("run a container");

    let mut lines = Vec::new();
    let followed = tokio::time::timeout(Duration::from_secs(20), async {
        let mut stream = Box::pin(client.follow_logs(&name, None));
        while let Some(line) = stream.next().await {
            lines.push(line.expect("line"));
        }
    })
    .await;
    assert!(
        followed.is_ok(),
        "the stream did not end when the container did"
    );

    let texts: Vec<_> = lines.iter().map(|l| l.text.as_str()).collect();
    assert!(texts.contains(&"line 5"), "{texts:?}");
    assert!(
        lines
            .iter()
            .any(|l| l.text == "oops 5" && l.stream == shared::logs::Stream::Stderr),
        "stderr stays distinguishable: {texts:?}"
    );
    assert!(
        lines.iter().all(|l| l.at.is_some()),
        "every line is timestamped"
    );
}

#[tokio::test]
async fn a_command_runs_and_reports_its_output_and_exit_code() {
    if docker(&["info", "--format", "{{.ServerVersion}}"]).is_none() {
        eprintln!("SKIPPED: no Docker daemon reachable");
        return;
    }
    let client = docker::Client::connect().expect("client");
    let name = format!("ghostdock-test-run-{}", std::process::id());
    let _cleanup = Removed(name.clone());
    docker(&["run", "-d", "--name", &name, "alpine:3.22", "sleep", "120"]).expect("run");

    let result = client
        .run_command(
            &name,
            "echo hello; echo oops >&2; exit 3",
            Duration::from_secs(10),
        )
        .await
        .expect("ran");
    assert_eq!(result.exit_code, Some(3));
    assert!(!result.timed_out && !result.truncated);
    let lines: Vec<_> = result
        .output
        .iter()
        .map(|l| (l.stream, l.text.as_str()))
        .collect();
    assert!(
        lines.contains(&(shared::logs::Stream::Stdout, "hello")),
        "{lines:?}"
    );
    assert!(
        lines.contains(&(shared::logs::Stream::Stderr, "oops")),
        "{lines:?}"
    );

    let slow = client
        .run_command(&name, "sleep 30", Duration::from_secs(1))
        .await
        .expect("ran");
    assert!(slow.timed_out);
    assert_eq!(slow.exit_code, None);
}

#[tokio::test]
async fn a_line_written_in_pieces_arrives_as_one() {
    if docker(&["info", "--format", "{{.ServerVersion}}"]).is_none() {
        eprintln!("SKIPPED: no Docker daemon reachable");
        return;
    }
    let client = docker::Client::connect().expect("client");
    let name = format!("ghostdock-test-pieces-{}", std::process::id());
    let _cleanup = Removed(name.clone());
    docker(&["run", "-d", "--name", &name, "alpine:3.22", "sleep", "120"]).expect("run");

    let result = client
        .run_command(
            &name,
            "printf 'half a '; sleep 0.5; printf 'line\\nno newline'",
            Duration::from_secs(10),
        )
        .await
        .expect("ran");
    let texts: Vec<_> = result.output.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["half a line", "no newline"]);

    // In a shell, output that never gets a newline is still shown.
    let shell = client.start_shell(&name, "/bin/sh").await.expect("shell");
    let (mut reader, mut writer) = shell.split();
    writer
        .write("printf 'no newline here'\n")
        .await
        .expect("write");
    let shown = tokio::time::timeout(Duration::from_secs(5), reader.next_lines())
        .await
        .expect("output within 5s")
        .expect("the shell is open");
    let texts: Vec<_> = shown.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["no newline here"]);
}

#[tokio::test]
async fn a_command_that_floods_its_output_is_cut_off() {
    if docker(&["info", "--format", "{{.ServerVersion}}"]).is_none() {
        eprintln!("SKIPPED: no Docker daemon reachable");
        return;
    }
    let client = docker::Client::connect().expect("client");
    let name = format!("ghostdock-test-flood-{}", std::process::id());
    let _cleanup = Removed(name.clone());
    docker(&["run", "-d", "--name", &name, "alpine:3.22", "sleep", "120"]).expect("run");

    let result = client
        .run_command(
            &name,
            "yes 'a fairly long line of output'",
            Duration::from_secs(5),
        )
        .await
        .expect("ran");
    assert!(
        result.truncated,
        "an endless command must be cut off, not buffered"
    );
    let bytes: usize = result.output.iter().map(|l| l.text.len()).sum();
    assert!(bytes <= docker::RUN_OUTPUT_LIMIT, "{bytes}");
}

#[tokio::test]
async fn a_busy_container_is_seen_to_use_cpu_and_its_limit_is_reported() {
    if docker(&["info", "--format", "{{.ServerVersion}}"]).is_none() {
        eprintln!("SKIPPED: no Docker daemon reachable");
        return;
    }
    let client = docker::Client::connect().expect("client");
    let name = format!("ghostdock-test-stats-{}", std::process::id());
    let _cleanup = Removed(name.clone());
    docker(&[
        "run",
        "-d",
        "--name",
        &name,
        "--memory",
        "64m",
        "alpine:3.22",
        "sh",
        "-c",
        "yes > /dev/null",
    ])
    .expect("run");

    let mut stream = Box::pin(client.stats(&name));
    let first = stream.next().await.expect("a reading").expect("ok");
    let started = std::time::Instant::now();
    let second = stream.next().await.expect("a reading").expect("ok");
    let reading = domain::metrics::rate(
        &first,
        &second,
        started.elapsed().max(Duration::from_millis(500)),
        None,
    )
    .expect("a rate");
    assert!(
        reading.cpu.unwrap() > 0.3,
        "yes keeps a core busy: {:?}",
        reading.cpu
    );
    assert_eq!(reading.mem_limit, Some(64 << 20));
}

#[tokio::test]
async fn a_containers_figures_end_when_it_stops() {
    // The daemon keeps streaming zeroed figures for a stopped container;
    // those must end the stream, not become readings of nothing.
    if docker(&["info", "--format", "{{.ServerVersion}}"]).is_none() {
        eprintln!("SKIPPED: no Docker daemon reachable");
        return;
    }
    let client = docker::Client::connect().expect("client");
    let name = format!("ghostdock-test-stats-stop-{}", std::process::id());
    let _cleanup = Removed(name.clone());
    docker(&["run", "-d", "--name", &name, "alpine:3.22", "sleep", "300"]).expect("run");

    let mut stream = Box::pin(client.stats(&name));
    stream.next().await.expect("a reading").expect("ok");
    docker(&["stop", "-t", "0", &name]).expect("stop");
    let ended = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(item) = stream.next().await {
            item.expect("ok");
        }
    })
    .await;
    assert!(ended.is_ok(), "the stream outlived its container");
}
