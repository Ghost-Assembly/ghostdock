//! Uptime checks and alerts, through the real router, against real local
//! listeners: a site the checks probe and a webhook the alerts reach.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::{get, post};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use server::app;
use server::state::AppState;
use shared::event::ServerEvent;
use store::Store;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";

struct Harness {
    router: Router,
    state: AppState,
    cookie: String,
    bearer: Option<String>,
    _root: tempfile::TempDir,
}

impl Harness {
    async fn start() -> Self {
        let store = Store::open_in_memory().await.unwrap();
        let metrics = store::metrics::MetricsStore::open_in_memory()
            .await
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        let state = AppState::new(store, None, root.path())
            .with_metrics(metrics)
            .with_alert_backoff(Duration::from_millis(10));
        let router = app::build(state.clone(), false, None);
        let mut h = Self {
            router,
            state,
            cookie: String::new(),
            bearer: None,
            _root: root,
        };
        let req = Request::post("/api/v1/auth/bootstrap")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "username": "admin", "password": PASSWORD }).to_string(),
            ))
            .unwrap();
        let res = h.router.clone().oneshot(req).await.unwrap();
        h.cookie = res.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        h
    }

    async fn send(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value, String) {
        let mut req = Request::builder().method(method).uri(uri);
        req = match &self.bearer {
            Some(token) => req.header("authorization", format!("Bearer {token}")),
            None => req.header("cookie", &self.cookie),
        };
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            text,
        )
    }

    /// The same server, as a token granted exactly `permissions`.
    async fn as_token(&self, permissions: &[&str]) -> Self {
        let (status, body, _) = self
            .send(
                "POST",
                "/api/v1/tokens",
                Some(json!({ "name": format!("t {permissions:?}"), "permissions": permissions, "expires_in_days": null })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        Self {
            router: self.router.clone(),
            state: self.state.clone(),
            cookie: String::new(),
            bearer: Some(body["secret"].as_str().unwrap().to_owned()),
            _root: tempfile::tempdir().unwrap(),
        }
    }

    async fn create_check(&self, body: Value) -> Value {
        let (status, made, _) = self
            .send("POST", "/api/v1/hosts/1/checks", Some(body))
            .await;
        assert_eq!(status, StatusCode::OK, "{made}");
        made
    }
}

/// Serves `router` on a free local port.
async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}

type Heard = Arc<Mutex<Vec<(HeaderMap, String)>>>;

/// A webhook that records what it is sent.
async fn webhook() -> (String, Heard) {
    let heard: Heard = Arc::default();
    let log = Arc::clone(&heard);
    let router = Router::new().route(
        "/hooks/{secret}",
        post(move |headers: HeaderMap, body: String| {
            let log = Arc::clone(&log);
            async move {
                log.lock().unwrap().push((headers, body));
                StatusCode::OK
            }
        }),
    );
    (serve(router).await, heard)
}

/// Waits until `heard` holds `n` messages.
async fn heard_at_least(heard: &Heard, n: usize) -> Vec<(HeaderMap, String)> {
    for _ in 0..200 {
        if heard.lock().unwrap().len() >= n {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    heard.lock().unwrap().clone()
}

// ---- checks -------------------------------------------------------------------

#[tokio::test]
async fn a_check_is_made_listed_changed_and_removed() {
    let h = Harness::start().await;
    let made = h
        .create_check(
            json!({ "name": "Blog", "kind": "http", "target": "https://blog.example.test/health" }),
        )
        .await;
    let id = made["check"]["id"].as_i64().unwrap();
    assert_eq!(made["check"]["interval_s"], json!(60));
    assert_eq!(made["status"]["state"], json!("pending"));
    assert_eq!(made["uptime_24h"], Value::Null, "no runs yet");

    let (status, list, _) = h.send("GET", "/api/v1/hosts/1/checks", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);

    let (status, changed, _) = h
        .send(
            "PUT",
            &format!("/api/v1/checks/{id}"),
            Some(json!({ "retries": 4, "enabled": false })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["check"]["retries"], json!(4));
    assert_eq!(
        changed["check"]["target"],
        json!("https://blog.example.test/health")
    );
    assert_eq!(changed["status"]["state"], json!("paused"));

    let (status, _, _) = h
        .send("DELETE", &format!("/api/v1/checks/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = h.send("GET", &format!("/api/v1/checks/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The trail names the check, never where it points.
    let (_, _, audit) = h.send("GET", "/api/v1/audit", None).await;
    for action in ["add check", "change check", "remove check"] {
        assert!(audit.contains(action), "{action}: {audit}");
    }
    assert!(!audit.contains("blog.example.test"), "{audit}");
}

#[tokio::test]
async fn settings_that_cannot_work_are_refused_with_a_reason() {
    let h = Harness::start().await;
    for (body, words) in [
        (
            json!({ "name": "x", "kind": "http", "target": "https://a.test/", "interval_s": 5 }),
            "20 seconds",
        ),
        (
            json!({ "name": "x", "kind": "tcp", "target": "a.test" }),
            "host:port",
        ),
        (
            json!({ "name": "x", "kind": "http", "target": "ftp://a.test/" }),
            "http",
        ),
        (
            json!({ "name": "x", "kind": "http", "target": "https://a.test/", "stack_id": 42 }),
            "stack",
        ),
        (
            json!({ "kind": "http", "target": "https://a.test/" }),
            "name",
        ),
    ] {
        let (status, answer, _) = h
            .send("POST", "/api/v1/hosts/1/checks", Some(body.clone()))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {answer}");
        assert!(
            answer["message"].as_str().unwrap().contains(words),
            "{body}: {answer}"
        );
    }
    h.create_check(json!({ "name": "Blog", "kind": "tcp", "target": "127.0.0.1:9" }))
        .await;
    let (status, _, _) = h
        .send(
            "POST",
            "/api/v1/hosts/1/checks",
            Some(json!({ "name": "Blog", "kind": "tcp", "target": "127.0.0.1:9" })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _, _) = h
        .send(
            "POST",
            "/api/v1/hosts/99/checks",
            Some(json!({ "name": "B", "kind": "tcp", "target": "127.0.0.1:9" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn viewing_needs_host_view_and_changing_needs_checks_manage() {
    let h = Harness::start().await;
    let made = h
        .create_check(json!({ "name": "Blog", "kind": "tcp", "target": "127.0.0.1:9" }))
        .await;
    let id = made["check"]["id"].as_i64().unwrap();

    let viewer = h.as_token(&["host.view"]).await;
    assert_eq!(
        viewer.send("GET", "/api/v1/hosts/1/checks", None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        viewer
            .send(
                "GET",
                &format!("/api/v1/checks/{id}/history?range=7d"),
                None
            )
            .await
            .0,
        StatusCode::OK
    );
    for (method, uri, body) in [
        (
            "POST",
            "/api/v1/hosts/1/checks".to_owned(),
            Some(json!({ "name": "B", "kind": "tcp", "target": "127.0.0.1:9" })),
        ),
        (
            "PUT",
            format!("/api/v1/checks/{id}"),
            Some(json!({ "retries": 3 })),
        ),
        ("DELETE", format!("/api/v1/checks/{id}"), None),
    ] {
        let (status, body, _) = viewer.send(method, &uri, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {body}");
        assert_eq!(body["code"], json!("missing_permission"));
    }

    let manager = h.as_token(&["checks.manage"]).await;
    assert_eq!(
        manager.send("GET", "/api/v1/hosts/1/checks", None).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, body, _) = manager
        .send(
            "PUT",
            &format!("/api/v1/checks/{id}"),
            Some(json!({ "retries": 3 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_history_range_is_one_of_the_known_ones() {
    let h = Harness::start().await;
    let made = h
        .create_check(json!({ "name": "Blog", "kind": "tcp", "target": "127.0.0.1:9" }))
        .await;
    let id = made["check"]["id"].as_i64().unwrap();
    let (status, _, _) = h
        .send(
            "GET",
            &format!("/api/v1/checks/{id}/history?range=forever"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, history, _) = h
        .send("GET", &format!("/api/v1/checks/{id}/history"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history["range"], json!("24h"));
    assert!(
        history["step"].as_i64().unwrap() >= 60,
        "never closer than the check runs"
    );
}

/// A site whose health endpoint answers with whatever status is set.
async fn site() -> (String, Arc<AtomicU16>) {
    let status = Arc::new(AtomicU16::new(503));
    let answer = Arc::clone(&status);
    let router = Router::new().route(
        "/health",
        get(move || {
            let answer = Arc::clone(&answer);
            async move {
                (
                    StatusCode::from_u16(answer.load(Ordering::SeqCst)).unwrap(),
                    "ok",
                )
            }
        }),
    );
    (serve(router).await, status)
}

#[tokio::test]
async fn a_failing_check_goes_down_opens_an_incident_alerts_and_comes_back() {
    let h = Harness::start().await;
    let (hook, heard) = webhook().await;
    let (base, answer) = site().await;
    let (status, _, _) = h
        .send(
            "POST",
            "/api/v1/alerts/channels",
            Some(
                json!({ "name": "ops", "kind": "webhook", "url": format!("{hook}/hooks/s3cret") }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let target = format!("{base}/health");
    let made = h
        .create_check(json!({ "name": "Blog", "kind": "http", "target": target, "retries": 2 }))
        .await;
    let id = made["check"]["id"].as_i64().unwrap();
    let mut events = h.state.runner.subscribe();

    // One failure is not an outage.
    let status = h.state.checks.run_now(id).await.unwrap();
    assert_eq!(status.state, shared::checks::CheckState::Pending);
    assert!(status.message.unwrap().contains("503"));
    // The second is.
    let status = h.state.checks.run_now(id).await.unwrap();
    assert_eq!(status.state, shared::checks::CheckState::Down);
    let event = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(ServerEvent::CheckChanged { check }) = events.recv().await {
                return check;
            }
        }
    })
    .await
    .expect("a CheckChanged event");
    assert_eq!(event.state, shared::checks::CheckState::Down);

    let (_, incidents, _) = h.send("GET", "/api/v1/hosts/1/incidents", None).await;
    assert_eq!(incidents.as_array().unwrap().len(), 1);
    assert_eq!(incidents[0]["ended_at"], Value::Null);
    assert!(incidents[0]["cause"].as_str().unwrap().contains("503"));

    let sent = heard_at_least(&heard, 1).await;
    let alert: Value = serde_json::from_str(&sent[0].1).unwrap();
    assert_eq!(alert["kind"], json!("check"));
    assert_eq!(alert["subject"], json!("Blog"));
    assert_eq!(alert["state"], json!("down"));
    assert_eq!(alert["url"], json!(target));

    // Back up: the incident closes and the channel hears it.
    answer.store(200, Ordering::SeqCst);
    let status = h.state.checks.run_now(id).await.unwrap();
    assert_eq!(status.state, shared::checks::CheckState::Up);
    let sent = heard_at_least(&heard, 2).await;
    let alert: Value = serde_json::from_str(&sent[1].1).unwrap();
    assert_eq!(alert["state"], json!("up"));
    let (_, incidents, _) = h.send("GET", "/api/v1/hosts/1/incidents", None).await;
    assert!(incidents[0]["ended_at"].is_string(), "{incidents}");

    // Every run is history.
    let (_, summary, _) = h.send("GET", &format!("/api/v1/checks/{id}"), None).await;
    assert_eq!(summary["status"]["state"], json!("up"));
    let uptime = summary["uptime_24h"].as_f64().unwrap();
    assert!((uptime - 1.0 / 3.0).abs() < 1e-9, "{summary}");
    assert_eq!(summary["recent"].as_array().unwrap().len(), 3);
    let (_, history, _) = h
        .send(
            "GET",
            &format!("/api/v1/checks/{id}/history?range=1h"),
            None,
        )
        .await;
    assert_eq!(history["incidents"].as_array().unwrap().len(), 1);
    let runs: u64 = history["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["total"].as_u64().unwrap())
        .sum();
    assert_eq!(runs, 3);

    // A delivery is recorded under the channel's name, never its URL.
    let (_, deliveries, text) = h.send("GET", "/api/v1/alerts/deliveries", None).await;
    assert_eq!(deliveries.as_array().unwrap().len(), 2, "{deliveries}");
    assert_eq!(deliveries[0]["channel"], json!("ops"));
    assert!(!text.contains("s3cret"), "{text}");
}

#[tokio::test]
async fn a_check_that_should_not_notify_does_not() {
    let h = Harness::start().await;
    let (hook, heard) = webhook().await;
    h.send(
        "POST",
        "/api/v1/alerts/channels",
        Some(json!({ "name": "ops", "kind": "webhook", "url": format!("{hook}/hooks/x") })),
    )
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed = listener.local_addr().unwrap().to_string();
    drop(listener);
    let made = h
        .create_check(json!({ "name": "Quiet", "kind": "tcp", "target": closed, "retries": 1, "notify": false }))
        .await;
    let status = h
        .state
        .checks
        .run_now(made["check"]["id"].as_i64().unwrap())
        .await
        .unwrap();
    assert_eq!(status.state, shared::checks::CheckState::Down);
    assert!(status.message.unwrap().to_lowercase().contains("refused"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(heard.lock().unwrap().is_empty());
}

/// Waits for check `id` to reach `state` by itself.
async fn reaches(h: &Harness, id: i64, state: &str) -> Value {
    let mut last = Value::Null;
    for _ in 0..100 {
        let (_, summary, _) = h.send("GET", &format!("/api/v1/checks/{id}"), None).await;
        if summary["status"]["state"] == json!(state) {
            return summary;
        }
        last = summary;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("never {state}: {last}");
}

#[tokio::test]
async fn once_started_a_check_runs_by_itself_and_again_when_changed() {
    let h = Harness::start().await;
    h.state.checks.start().await;
    let open = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let made = h
        .create_check(json!({ "name": "Db", "kind": "tcp", "target": open.local_addr().unwrap().to_string() }))
        .await;
    let id = made["check"]["id"].as_i64().unwrap();
    let up = reaches(&h, id, "up").await;
    assert!(up["status"]["latency_ms"].is_u64(), "{up}");

    // Pointed somewhere closed, it starts again and, allowed one failure,
    // goes down at its first run.
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = closed.local_addr().unwrap().to_string();
    drop(closed);
    let (status, changed, _) = h
        .send(
            "PUT",
            &format!("/api/v1/checks/{id}"),
            Some(json!({ "target": target, "retries": 1 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        changed["status"]["state"],
        json!("pending"),
        "an edit starts it again"
    );
    reaches(&h, id, "down").await;

    // Turned off, it is paused and stays so.
    h.send(
        "PUT",
        &format!("/api/v1/checks/{id}"),
        Some(json!({ "enabled": false })),
    )
    .await;
    reaches(&h, id, "paused").await;
    drop(open);
}

#[tokio::test]
async fn a_container_check_without_a_daemon_is_down_and_says_why() {
    let h = Harness::start().await;
    let made = h
        .create_check(
            json!({ "name": "Web", "kind": "container", "target": "blog-web-1", "retries": 1 }),
        )
        .await;
    let status = h
        .state
        .checks
        .run_now(made["check"]["id"].as_i64().unwrap())
        .await
        .unwrap();
    assert_eq!(status.state, shared::checks::CheckState::Down);
    assert!(status.message.unwrap().contains("Docker"));
}

fn docker_cli(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("docker")
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// A container this test made, removed when the test ends however it ends.
struct Fixture(String);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = docker_cli(&["rm", "-f", &self.0]);
    }
}

#[tokio::test]
async fn a_container_check_follows_docker_and_hears_a_stop_at_once() {
    if docker_cli(&["info", "--format", "{{.ServerVersion}}"]).is_none() {
        eprintln!("SKIPPED: no Docker daemon reachable");
        return;
    }
    let name = format!("ghostdocktest-check-{}", std::process::id());
    let _ = docker_cli(&["rm", "-f", &name]);
    docker_cli(&[
        "run",
        "-d",
        "--name",
        &name,
        "alpine:3.22",
        "sh",
        "-c",
        "sleep 600",
    ])
    .expect("run a container");
    let fixture = Fixture(name.clone());

    let store = Store::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let client = docker::Client::connect().expect("client");
    let state = AppState::new(store, Some(client.clone()), root.path());
    server::watch::spawn(client, state.runner.clone());
    let router = app::build(state.clone(), false, None);
    let mut h = Harness {
        router,
        state,
        cookie: String::new(),
        bearer: None,
        _root: root,
    };
    let req = Request::post("/api/v1/auth/bootstrap")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "username": "admin", "password": PASSWORD }).to_string(),
        ))
        .unwrap();
    let res = h.router.clone().oneshot(req).await.unwrap();
    h.cookie = res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();

    h.state.checks.start().await;
    let made = h
        .create_check(json!({ "name": "Box", "kind": "container", "target": name, "retries": 1, "interval_s": 3600 }))
        .await;
    let id = made["check"]["id"].as_i64().unwrap();
    reaches(&h, id, "up").await;

    // Its interval is an hour: only Docker's own event can bring this.
    docker_cli(&["stop", "-t", "0", &fixture.0]).expect("stop the container");
    let down = reaches(&h, id, "down").await;
    assert!(
        down["status"]["message"]
            .as_str()
            .unwrap()
            .contains("stopped"),
        "{down}"
    );
}

// ---- channels -----------------------------------------------------------------

#[tokio::test]
async fn a_channel_url_and_token_never_come_back_out() {
    let h = Harness::start().await;
    let (hook, heard) = webhook().await;
    let url = format!("{hook}/hooks/url-secret-123");
    let (status, made, text) = h
        .send(
            "POST",
            "/api/v1/alerts/channels",
            Some(json!({ "name": "phone", "kind": "ntfy", "url": url, "token": "tk_token-secret-456" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    assert_eq!(made["host"], json!(hook.trim_start_matches("http://")));
    let id = made["id"].as_i64().unwrap();

    let (_, _, listed) = h.send("GET", "/api/v1/alerts/channels", None).await;
    let (status, delivery, tested) = h
        .send("POST", &format!("/api/v1/alerts/channels/{id}/test"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{delivery}");
    assert_eq!(delivery["ok"], json!(true));
    let (_, _, deliveries) = h.send("GET", "/api/v1/alerts/deliveries", None).await;
    let (_, _, audit) = h.send("GET", "/api/v1/audit", None).await;
    for (what, body) in [
        ("created", &text),
        ("listed", &listed),
        ("tested", &tested),
        ("deliveries", &deliveries),
        ("audit", &audit),
    ] {
        assert!(!body.contains("url-secret-123"), "{what}: {body}");
        assert!(!body.contains("token-secret-456"), "{what}: {body}");
    }
    assert!(
        audit.contains("add alert channel") && audit.contains("phone"),
        "{audit}"
    );

    // ntfy gets the words and headers, and the token as a bearer.
    let sent = heard_at_least(&heard, 1).await;
    let (headers, body) = &sent[0];
    assert!(body.contains("It works"), "{body}");
    assert!(headers["title"].to_str().unwrap().contains("phone"));
    assert_eq!(headers["priority"], "low");
    assert_eq!(headers["authorization"], "Bearer tk_token-secret-456");

    let (status, _, _) = h
        .send("DELETE", &format!("/api/v1/alerts/channels/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = h
        .send("POST", &format!("/api/v1/alerts/channels/{id}/test"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_channel_must_be_a_web_address_with_a_name() {
    let h = Harness::start().await;
    for body in [
        json!({ "name": "x", "kind": "webhook", "url": "mailto:ops@example.test" }),
        json!({ "name": " ", "kind": "webhook", "url": "https://hooks.example.test/x" }),
        json!({ "name": "x", "kind": "ntfy", "url": "https://ntfy.example.test/x", "token": "a b" }),
    ] {
        let (status, answer, _) = h
            .send("POST", "/api/v1/alerts/channels", Some(body.clone()))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {answer}");
    }
}

#[tokio::test]
async fn a_delivery_is_retried_and_its_failure_recorded_without_the_url() {
    let h = Harness::start().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed = listener.local_addr().unwrap();
    drop(listener);
    h.send(
        "POST",
        "/api/v1/alerts/channels",
        Some(json!({ "name": "gone", "kind": "webhook", "url": format!("http://{closed}/hooks/shh-secret") })),
    )
    .await;
    let alert = shared::alerts::Alert {
        kind: shared::alerts::AlertKind::Check,
        subject: "Blog".to_owned(),
        state: "down".to_owned(),
        message: "Blog is down".to_owned(),
        at: chrono::Utc::now(),
        url: None,
    };
    let done = h.state.alerts.deliver_all(&alert).await;
    assert_eq!(done.len(), 1);
    assert!(!done[0].ok);
    assert_eq!(done[0].attempts, 4, "the first and three retries");
    let error = done[0].error.clone().unwrap();
    assert!(!error.contains("shh-secret"), "{error}");
}

// ---- rules --------------------------------------------------------------------

#[tokio::test]
async fn a_resource_rule_fires_once_and_resolves_once() {
    let h = Harness::start().await;
    let (hook, heard) = webhook().await;
    h.send(
        "POST",
        "/api/v1/alerts/channels",
        Some(json!({ "name": "ops", "kind": "webhook", "url": format!("{hook}/hooks/x") })),
    )
    .await;
    let (status, rule, _) = h
        .send(
            "POST",
            "/api/v1/alerts/rules",
            Some(json!({ "subject": "host", "metric": "cpu", "above_pct": 50, "for_min": 2 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{rule}");

    let minute = |t: i64, cpu: f64| server::metrics::Minute {
        t,
        rows: vec![server::metrics::MinuteRow {
            kind: shared::metrics::SubjectKind::Host,
            key: "host".to_owned(),
            project: None,
            service: None,
            reading: shared::metrics::Reading {
                t,
                cpu: Some(cpu),
                ..shared::metrics::Reading::default()
            },
        }],
        cpus: Some(4),
        memory: Some(1 << 30),
    };
    for (t, cpu) in [(60, 3.0), (120, 3.5), (180, 3.9)] {
        h.state.alerts.evaluate(&minute(t, cpu)).await;
    }
    let sent = heard_at_least(&heard, 1).await;
    let (_, rules, _) = h.send("GET", "/api/v1/alerts/rules", None).await;
    assert_eq!(rules[0]["firing"], json!(true));
    h.state.alerts.evaluate(&minute(240, 0.5)).await;
    let sent_after = heard_at_least(&heard, 2).await;
    assert_eq!(sent.len(), 1, "once, not every minute over");
    let states: Vec<String> = sent_after
        .iter()
        .map(|(_, body)| {
            serde_json::from_str::<Value>(body).unwrap()["state"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(states, ["firing", "resolved"]);
    let fired: Value = serde_json::from_str(&sent_after[0].1).unwrap();
    assert_eq!(fired["kind"], json!("resource"));
    assert!(
        fired["message"].as_str().unwrap().contains("CPU above 50%"),
        "{fired}"
    );
}

#[tokio::test]
async fn a_rule_must_watch_something_that_exists() {
    let h = Harness::start().await;
    for body in [
        json!({ "subject": "container:web", "metric": "disk", "above_pct": 90, "for_min": 5 }),
        json!({ "subject": "stack:42", "metric": "cpu", "above_pct": 90, "for_min": 5 }),
        json!({ "subject": "host", "metric": "cpu", "above_pct": 190, "for_min": 5 }),
    ] {
        let (status, answer, _) = h
            .send("POST", "/api/v1/alerts/rules", Some(body.clone()))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {answer}");
    }
    let (status, _, _) = h
        .send(
            "PUT",
            "/api/v1/alerts/rules/77",
            Some(json!({ "subject": "host", "metric": "cpu", "above_pct": 90, "for_min": 5 })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn alerts_are_viewed_with_host_view_and_changed_with_alerts_manage() {
    let h = Harness::start().await;
    let viewer = h.as_token(&["host.view"]).await;
    for uri in [
        "/api/v1/alerts/channels",
        "/api/v1/alerts/rules",
        "/api/v1/alerts/deliveries",
    ] {
        assert_eq!(
            viewer.send("GET", uri, None).await.0,
            StatusCode::OK,
            "{uri}"
        );
    }
    let (status, _, _) = viewer
        .send(
            "POST",
            "/api/v1/alerts/channels",
            Some(json!({ "name": "x", "kind": "webhook", "url": "https://h.test/x" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let manager = h.as_token(&["alerts.manage"]).await;
    let (status, made, _) = manager
        .send(
            "POST",
            "/api/v1/alerts/channels",
            Some(json!({ "name": "x", "kind": "webhook", "url": "https://h.test/x" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    assert_eq!(
        manager.send("GET", "/api/v1/alerts/channels", None).await.0,
        StatusCode::FORBIDDEN
    );
}
