//! Reading and following several containers' output at once, against a
//! real Docker daemon. Skips loudly when none is reachable.
//!
//! Every container here is created by the test, named
//! `ghostdocktest-multilog-*` with this process's id, and removed by it;
//! nothing else on the host is touched.

use std::process::Command;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::StreamExt as _;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use server::app;
use server::state::AppState;
use shared::logs::{LiveLog, MergedLogs, TaggedLine};
use store::Store;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn docker(args: &[&str]) -> Option<String> {
    let out = Command::new("docker").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn daemon_available() -> bool {
    docker(&["info", "--format", "{{.ServerVersion}}"]).is_some()
}

/// A container this test made, removed when the test ends however it ends.
struct Fixture(String);

impl Fixture {
    /// Runs `script` with `sh -c` in a new container called
    /// `ghostdocktest-multilog-<what>-<pid>`, with `labels`.
    fn run(what: &str, labels: &[&str], script: &str) -> Self {
        let name = format!("ghostdocktest-multilog-{what}-{}", std::process::id());
        let _ = docker(&["rm", "-f", &name]);
        let mut args = vec!["run", "-d", "--name", &name];
        for label in labels {
            args.extend(["--label", label]);
        }
        args.extend(["alpine:3.22", "sh", "-c", script]);
        docker(&args).expect("run a container");
        Self(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = docker(&["rm", "-f", &self.0]);
    }
}

struct Served {
    addr: std::net::SocketAddr,
    router: axum::Router,
    cookie: String,
    _root: tempfile::TempDir,
}

async fn serve() -> Served {
    let store = Store::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let docker = docker::Client::connect().expect("client");
    let state = AppState::new(store, Some(docker), root.path());
    let router = app::build(state, false, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let served = router.clone();
    tokio::spawn(async move { axum::serve(listener, served).await });
    let (_, _, cookie) = call(
        &router,
        "POST",
        "/api/v1/auth/bootstrap",
        None,
        json!({ "username": "admin", "password": PASSWORD }),
    )
    .await;
    Served {
        addr,
        router,
        cookie: cookie.expect("a session"),
        _root: root,
    }
}

async fn call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Value,
) -> (StatusCode, axum::body::Bytes, Option<String>) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    let res = router
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let set = res
        .headers()
        .get("set-cookie")
        .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_owned());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, bytes, set)
}

impl Served {
    async fn get(&self, uri: &str) -> (StatusCode, axum::body::Bytes) {
        let (status, body, _) =
            call(&self.router, "GET", uri, Some(&self.cookie), Value::Null).await;
        (status, body)
    }

    async fn json(&self, method: &str, uri: &str, body: Value) -> Value {
        let (status, body, _) = call(&self.router, method, uri, Some(&self.cookie), body).await;
        assert!(status.is_success(), "{method} {uri}: {status}");
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    }

    /// A token holding `permissions`, and its id.
    async fn token(&self, permissions: &[&str]) -> (String, i64) {
        let body = self
            .json(
                "POST",
                "/api/v1/tokens",
                json!({ "name": "multilog", "permissions": permissions, "expires_in_days": null }),
            )
            .await;
        (
            body["secret"].as_str().unwrap().to_owned(),
            body["token"]["id"]
                .as_i64()
                .or_else(|| body["id"].as_i64())
                .expect("the token's id"),
        )
    }

    async fn connect(&self, path_and_query: &str, bearer: Option<&str>) -> Socket {
        let mut request = format!("ws://{}{path_and_query}", self.addr)
            .into_client_request()
            .unwrap();
        let header = match bearer {
            Some(secret) => ("authorization", format!("Bearer {secret}")),
            None => ("cookie", self.cookie.clone()),
        };
        request
            .headers_mut()
            .insert(header.0, header.1.parse().unwrap());
        tokio_tungstenite::connect_async(request)
            .await
            .expect("connects")
            .0
    }
}

/// Reads lines from `ws` until `done` says enough, or `within` passes.
async fn read_until(
    ws: &mut Socket,
    within: Duration,
    mut done: impl FnMut(&[TaggedLine]) -> bool,
) -> Vec<TaggedLine> {
    let mut seen = Vec::new();
    let _ = tokio::time::timeout(within, async {
        while let Some(Ok(message)) = ws.next().await {
            if let Message::Text(text) = message
                && let Ok(LiveLog::Line(line)) = serde_json::from_str::<LiveLog>(&text)
            {
                seen.push(line);
                if done(&seen) {
                    return;
                }
            }
        }
    })
    .await;
    seen
}

fn instant(line: &TaggedLine) -> chrono::DateTime<chrono::FixedOffset> {
    chrono::DateTime::parse_from_rfc3339(line.at.as_deref().expect("stamped")).expect("a time")
}

/// Five lines each, a fifth of a second apart, then quiet.
fn five(tag: &str) -> String {
    format!("i=0; while [ $i -lt 5 ]; do echo {tag} $i; i=$((i+1)); sleep 0.2; done; sleep 600")
}

#[tokio::test]
async fn a_snapshot_merges_two_containers_in_time_order() {
    if !daemon_available() {
        eprintln!("SKIPPED: no Docker daemon reachable");
        return;
    }
    let a = Fixture::run("snap-a", &[], &five("alpha"));
    let b = Fixture::run("snap-b", &[], &five("beta"));
    tokio::time::sleep(Duration::from_secs(2)).await;
    let s = serve().await;

    let (status, body) = s
        .get(&format!("/api/v1/hosts/1/logs?containers={},{}", a.0, b.0))
        .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let logs: MergedLogs = serde_json::from_slice(&body).unwrap();
    assert_eq!(logs.containers, [a.0.clone(), b.0.clone()]);
    assert_eq!(logs.lines.len(), 10, "{:?}", logs.lines);
    assert!(!logs.truncated);
    for pair in logs.lines.windows(2) {
        assert!(instant(&pair[0]) <= instant(&pair[1]), "{pair:?}");
    }
    for line in &logs.lines {
        let from_a = line.text.starts_with("alpha");
        assert_eq!(
            &line.container,
            if from_a { &a.0 } else { &b.0 },
            "{line:?}"
        );
        assert_eq!(line.stack, None);
    }
    let a_order: Vec<&str> = logs
        .lines
        .iter()
        .filter(|l| l.container == a.0)
        .map(|l| l.text.as_str())
        .collect();
    assert_eq!(
        a_order,
        ["alpha 0", "alpha 1", "alpha 2", "alpha 3", "alpha 4"]
    );

    // Case-insensitive, and the newest per container when the tail is short.
    let (_, body) = s
        .get(&format!(
            "/api/v1/hosts/1/logs?containers={},{}&contains=ALPHA%204",
            a.0, b.0
        ))
        .await;
    let found: MergedLogs = serde_json::from_slice(&body).unwrap();
    let texts: Vec<&str> = found.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["alpha 4"]);
    let (_, body) = s
        .get(&format!(
            "/api/v1/hosts/1/logs?containers={},{}&tail=2",
            a.0, b.0
        ))
        .await;
    let short: MergedLogs = serde_json::from_slice(&body).unwrap();
    assert_eq!(short.lines.len(), 4);
    assert!(short.truncated);

    // The same as a file.
    let (status, body) = s
        .get(&format!(
            "/api/v1/hosts/1/logs.txt?containers={},{}",
            a.0, b.0
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8_lossy(&body);
    assert_eq!(text.lines().count(), 10, "{text}");
    assert!(text.contains(&format!("{} alpha 0", a.0)), "{text}");
    assert!(text.contains(&format!("{} beta 4", b.0)), "{text}");

    // A name the daemon does not have is said, not skipped.
    let (status, body) = s
        .get(&format!(
            "/api/v1/hosts/1/logs?containers={},ghostdocktest-multilog-none",
            a.0
        ))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&body).contains("ghostdocktest-multilog-none"));
}

/// A line every fifth of a second, for as long as it runs.
fn chatter(tag: &str) -> String {
    format!("i=0; while :; do echo {tag} $i; i=$((i+1)); sleep 0.2; done")
}

#[tokio::test]
async fn one_socket_follows_two_containers_until_the_token_is_revoked() {
    if !daemon_available() {
        eprintln!("SKIPPED: no Docker daemon reachable");
        return;
    }
    let a = Fixture::run("live-a", &[], &chatter("alpha"));
    let b = Fixture::run("live-b", &[], &chatter("beta"));
    let s = serve().await;
    let (secret, id) = s.token(&["logs.view"]).await;
    let mut ws = s
        .connect(
            &format!("/api/v1/hosts/1/logs/socket?containers={},{}", a.0, b.0),
            Some(&secret),
        )
        .await;

    let seen = read_until(&mut ws, Duration::from_secs(15), |seen| {
        seen.iter().any(|l| l.container == a.0) && seen.iter().any(|l| l.container == b.0)
    })
    .await;
    assert!(
        seen.iter()
            .any(|l| l.container == a.0 && l.text.starts_with("alpha")),
        "{seen:?}"
    );
    assert!(
        seen.iter()
            .any(|l| l.container == b.0 && l.text.starts_with("beta")),
        "{seen:?}"
    );

    s.json("DELETE", &format!("/api/v1/tokens/{id}"), Value::Null)
        .await;
    let reason = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(message)) = ws.next().await {
            if let Message::Close(frame) = message {
                return frame.map(|f| f.reason.to_string());
            }
        }
        None
    })
    .await;
    assert_eq!(
        reason.ok().flatten().as_deref(),
        Some("revoked"),
        "the socket outlived its token"
    );
}

#[tokio::test]
async fn a_stack_socket_picks_up_a_container_that_starts_later() {
    if !daemon_available() {
        eprintln!("SKIPPED: no Docker daemon reachable");
        return;
    }
    let s = serve().await;
    let name = format!("ghostdocktest-multilog-stack-{}", std::process::id());
    let stack = s
        .json(
            "POST",
            "/api/v1/hosts/1/stacks",
            json!({ "name": name, "compose_yaml": "services:\n  x:\n    image: alpine:3.22\n" }),
        )
        .await;
    let (id, slug) = (
        stack["id"].as_i64().unwrap(),
        stack["slug"].as_str().unwrap().to_owned(),
    );
    let mut ws = s
        .connect(&format!("/api/v1/hosts/1/logs/socket?stack={id}"), None)
        .await;
    // The socket is surely watching for starts by now.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let label = format!("com.docker.compose.project={slug}");
    let late = Fixture::run("late", &[&label], &chatter("late"));
    let seen = read_until(&mut ws, Duration::from_secs(15), |seen| !seen.is_empty()).await;
    let first = seen
        .first()
        .expect("a line from the container that started");
    assert_eq!(first.container, late.0);
    assert_eq!(first.stack.as_deref(), Some(slug.as_str()));
    assert!(first.text.starts_with("late"), "{first:?}");
}

#[tokio::test]
async fn a_reader_that_falls_behind_is_told_how_much_it_missed() {
    if !daemon_available() {
        eprintln!("SKIPPED: no Docker daemon reachable");
        return;
    }
    // Waits a moment so the socket is following before the flood starts.
    let flood = Fixture::run(
        "flood",
        &[],
        "sleep 1; yes 'the same line again' | head -n 200000; sleep 600",
    );
    let s = serve().await;
    let mut ws = s
        .connect(
            &format!("/api/v1/hosts/1/logs/socket?containers={}", flood.0),
            None,
        )
        .await;
    // Not reading: the socket's buffers fill, then the server's queue.
    tokio::time::sleep(Duration::from_secs(6)).await;

    let skipped = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(Ok(message)) = ws.next().await {
            if let Message::Text(text) = message
                && let Ok(LiveLog::Skipped { skipped }) = serde_json::from_str::<LiveLog>(&text)
            {
                return skipped;
            }
        }
        0
    })
    .await;
    let skipped = skipped.expect("word of skipped lines within 30s");
    assert!(skipped > 0);
    // Still following afterwards.
    let more = read_until(&mut ws, Duration::from_secs(10), |seen| !seen.is_empty()).await;
    assert!(!more.is_empty(), "the socket stopped after skipping");
}

#[tokio::test]
async fn the_mcp_tool_reads_the_same_merged_output() {
    if !daemon_available() {
        eprintln!("SKIPPED: no Docker daemon reachable");
        return;
    }
    let a = Fixture::run("mcp-a", &[], &five("alpha"));
    let b = Fixture::run("mcp-b", &[], &five("beta"));
    tokio::time::sleep(Duration::from_secs(2)).await;
    let s = serve().await;
    let (secret, _) = s.token(&["logs.view"]).await;

    let message = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "logs_across", "arguments": {
            "containers": [a.0, b.0], "contains": "4",
        } },
    });
    let req = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("authorization", format!("Bearer {secret}"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Body::from(message.to_string()))
        .unwrap();
    let res = s.router.clone().oneshot(req).await.unwrap();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["result"]["isError"], json!(false), "{body}");
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    let lines: Vec<&str> = text.lines().skip(1).collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(lines[0].starts_with(&format!("{} out ", a.0)), "{text}");
    assert!(lines[0].ends_with("alpha 4"), "{text}");
    assert!(lines[1].starts_with(&format!("{} out ", b.0)), "{text}");
    assert!(lines[1].ends_with("beta 4"), "{text}");
}
