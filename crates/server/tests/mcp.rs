//! The MCP endpoint: Streamable HTTP, stateless, bearer tokens only.
//!
//! Tools reach GhostDock through its own API with the caller's token, so what
//! a tool may do is decided by exactly the rules the API already enforces.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use server::app;
use server::state::AppState;
use store::Store;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";
const COMPOSE: &str = "services:\n  web:\n    image: nginx:alpine\n";
static NEXT_TOKEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct Setup {
    router: Router,
    cookie: String,
    _root: tempfile::TempDir,
}

async fn call(
    router: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let req = match body {
        Some(b) => req.body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = router.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn setup() -> Setup {
    let store = Store::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let router = app::build(AppState::new(store, None, root.path()), false, None);
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/bootstrap")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "username": "admin", "password": PASSWORD }).to_string(),
        ))
        .unwrap();
    let res = router.clone().oneshot(req).await.unwrap();
    let cookie = res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    Setup {
        router,
        cookie,
        _root: root,
    }
}

impl Setup {
    async fn api(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (status, _, body) = call(
            &self.router,
            method,
            uri,
            &[
                ("cookie", &self.cookie),
                ("content-type", "application/json"),
            ],
            body,
        )
        .await;
        (status, body)
    }

    async fn token(&self, permissions: &[&str]) -> String {
        let (status, body) = self
            .api(
                "POST",
                "/api/v1/tokens",
                Some(json!({
                    "name": format!("mcp {}", NEXT_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)),
                    "permissions": permissions,
                    "expires_in_days": null,
                })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["secret"].as_str().unwrap().to_owned()
    }

    /// One MCP message, as Claude Code sends it.
    async fn mcp(&self, token: &str, message: Value) -> (StatusCode, axum::http::HeaderMap, Value) {
        let auth = format!("Bearer {token}");
        call(
            &self.router,
            "POST",
            "/mcp",
            &[
                ("authorization", &auth),
                ("content-type", "application/json"),
                ("accept", "application/json, text/event-stream"),
            ],
            Some(message),
        )
        .await
    }

    async fn rpc(&self, token: &str, method: &str, params: Value) -> Value {
        let (status, _, body) = self
            .mcp(
                token,
                json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{method}: {body}");
        assert_eq!(body["jsonrpc"], json!("2.0"));
        assert_eq!(body["id"], json!(1));
        body
    }

    async fn tool(&self, token: &str, name: &str, arguments: Value) -> Value {
        self.rpc(
            token,
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
        .await
    }
}

fn tool_names(list: &Value) -> Vec<String> {
    list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

// ---- transport ------------------------------------------------------------

#[tokio::test]
async fn without_a_token_the_answer_is_401_with_a_bearer_challenge() {
    let s = setup().await;
    let (status, headers, _) = call(
        &s.router,
        "POST",
        "/mcp",
        &[
            ("content-type", "application/json"),
            ("accept", "application/json"),
        ],
        Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        headers["www-authenticate"]
            .to_str()
            .unwrap()
            .starts_with("Bearer")
    );

    // A browser session is not enough: MCP is for programs, with tokens.
    let (status, _, _) = call(
        &s.router,
        "POST",
        "/mcp",
        &[
            ("cookie", &s.cookie),
            ("content-type", "application/json"),
            ("accept", "application/json"),
        ],
        Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn initialize_agrees_a_version_and_describes_the_server() {
    let s = setup().await;
    let token = s.token(&["host.view"]).await;
    let body = s
        .rpc(
            &token,
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" },
            }),
        )
        .await;
    assert_eq!(body["result"]["protocolVersion"], json!("2025-06-18"));
    assert_eq!(body["result"]["serverInfo"]["name"], json!("ghostdock"));
    assert!(body["result"]["capabilities"]["tools"].is_object());
    assert!(body["result"]["instructions"].is_string());

    // A version it does not know gets its newest, for the client to judge.
    let body = s
        .rpc(
            &token,
            "initialize",
            json!({ "protocolVersion": "2099-01-01", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } }),
        )
        .await;
    assert_eq!(body["result"]["protocolVersion"], json!("2025-11-25"));
}

#[tokio::test]
async fn a_notification_is_accepted_without_a_reply() {
    let s = setup().await;
    let token = s.token(&["host.view"]).await;
    let (status, _, body) = s
        .mcp(
            &token,
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body, Value::Null);
}

#[tokio::test]
async fn ping_answers_and_unknown_methods_are_named() {
    let s = setup().await;
    let token = s.token(&["host.view"]).await;
    let body = s.rpc(&token, "ping", json!({})).await;
    assert_eq!(body["result"], json!({}));

    let body = s.rpc(&token, "resources/list", json!({})).await;
    assert_eq!(body["error"]["code"], json!(-32601));
}

#[tokio::test]
async fn the_transport_is_strict_about_what_it_is_sent() {
    let s = setup().await;
    let token = s.token(&["host.view"]).await;
    let auth = format!("Bearer {token}");
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });

    // No server-initiated stream is offered.
    let (status, _, _) = call(&s.router, "GET", "/mcp", &[("authorization", &auth)], None).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

    let (status, _, _) = call(
        &s.router,
        "POST",
        "/mcp",
        &[
            ("authorization", &auth),
            ("content-type", "text/plain"),
            ("accept", "application/json"),
        ],
        Some(ping.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let (status, _, _) = call(
        &s.router,
        "POST",
        "/mcp",
        &[
            ("authorization", &auth),
            ("content-type", "application/json"),
            ("accept", "text/html"),
        ],
        Some(ping.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_ACCEPTABLE);

    let (status, _, body) = call(
        &s.router,
        "POST",
        "/mcp",
        &[
            ("authorization", &auth),
            ("content-type", "application/json"),
            ("accept", "application/json"),
            ("mcp-protocol-version", "1999-01-01"),
        ],
        Some(ping.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let req = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("authorization", &auth)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .body(Body::from("{not json"))
        .unwrap();
    let res = s.router.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], json!(-32700));
}

#[tokio::test]
async fn a_page_on_another_origin_cannot_use_it() {
    // Tokens are not sent by browsers on their own, but a page that has one
    // must still not reach GhostDock from elsewhere: MCP servers check Origin
    // against DNS rebinding.
    let s = setup().await;
    let token = s.token(&["host.view"]).await;
    let auth = format!("Bearer {token}");
    let (status, _, _) = call(
        &s.router,
        "POST",
        "/mcp",
        &[
            ("authorization", &auth),
            ("content-type", "application/json"),
            ("accept", "application/json"),
            ("origin", "https://evil.example"),
            ("host", "ghostdock.local:8080"),
        ],
        Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---- tools ----------------------------------------------------------------

#[tokio::test]
async fn a_token_sees_only_the_tools_its_permissions_allow() {
    let s = setup().await;
    let viewer = s.token(&["host.view"]).await;
    let names = tool_names(&s.rpc(&viewer, "tools/list", json!({})).await);
    assert!(names.contains(&"list_stacks".to_owned()), "{names:?}");
    assert!(names.contains(&"get_stack".to_owned()));
    assert!(!names.contains(&"deploy_stack".to_owned()), "{names:?}");
    assert!(!names.contains(&"run_command".to_owned()));

    let deployer = s.token(&["host.view", "stacks.deploy"]).await;
    let names = tool_names(&s.rpc(&deployer, "tools/list", json!({})).await);
    assert!(names.contains(&"deploy_stack".to_owned()));
    assert!(!names.contains(&"take_down_stack".to_owned()));
}

#[tokio::test]
async fn every_tool_is_described_well_enough_to_be_used() {
    let s = setup().await;
    let all = s
        .token(&[
            "host.view",
            "stacks.read_compose",
            "logs.view",
            "activity.view",
            "stacks.deploy",
            "stacks.restart",
            "stacks.stop",
            "stacks.take_down",
            "updates.check",
            "updates.auto_apply",
            "stacks.create",
            "stacks.edit",
            "stacks.forget",
            "env.write",
            "repos.manage",
            "credentials.manage",
            "cleanup.run",
            "shell.open",
        ])
        .await;
    let list = s.rpc(&all, "tools/list", json!({})).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    assert!(tools.len() >= 20, "{}", tools.len());
    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        assert!(tool["description"].as_str().unwrap().len() > 20, "{name}");
        assert_eq!(tool["inputSchema"]["type"], json!("object"), "{name}");
        assert!(tool["annotations"]["readOnlyHint"].is_boolean(), "{name}");
    }
    let destructive = |n: &str| {
        tools.iter().find(|t| t["name"] == json!(n)).unwrap()["annotations"]["destructiveHint"]
            == json!(true)
    };
    assert!(destructive("take_down_stack"));
    assert!(destructive("run_command"));
    assert!(!destructive("list_stacks"));
}

#[tokio::test]
async fn a_tool_the_token_may_not_use_does_not_exist_for_it() {
    let s = setup().await;
    let viewer = s.token(&["host.view"]).await;
    let body = s
        .tool(&viewer, "deploy_stack", json!({ "stack": "blog" }))
        .await;
    assert_eq!(body["error"]["code"], json!(-32602), "{body}");
}

#[tokio::test]
async fn a_token_allowed_only_to_edit_can_edit_a_compose_file() {
    // Nothing beyond stacks.edit should be needed to do what it names.
    let s = setup().await;
    let (_, created) = s
        .api(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(json!({ "name": "Blog", "compose_yaml": COMPOSE })),
        )
        .await;
    let id = created["id"].as_i64().unwrap();
    let editor = s.token(&["stacks.edit"]).await;

    let changed = "services:\n  web:\n    image: nginx:1.29-alpine\n";
    let body = s
        .tool(
            &editor,
            "update_compose",
            json!({ "stack": id.to_string(), "compose_yaml": changed }),
        )
        .await;
    assert_eq!(body["result"]["isError"], json!(false), "{body}");
    let (_, compose) = s
        .api("GET", &format!("/api/v1/stacks/{id}/compose"), None)
        .await;
    assert_eq!(
        compose["compose_yaml"].as_str().map(str::trim),
        Some(changed.trim())
    );
}

#[tokio::test]
async fn the_activity_tool_offers_no_more_than_the_api_returns() {
    let s = setup().await;
    let token = s.token(&["activity.view"]).await;
    let list = s.rpc(&token, "tools/list", json!({})).await;
    let activity = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == json!("activity"))
        .unwrap();
    assert_eq!(
        activity["inputSchema"]["properties"]["limit"]["maximum"],
        json!(200)
    );
}

#[tokio::test]
async fn tools_act_through_the_api_with_the_callers_token() {
    let s = setup().await;
    let token = s
        .token(&["host.view", "stacks.create", "stacks.read_compose"])
        .await;

    let body = s
        .tool(
            &token,
            "create_stack",
            json!({ "name": "Blog", "compose_yaml": COMPOSE }),
        )
        .await;
    assert_eq!(body["result"]["isError"], json!(false), "{body}");
    assert_eq!(body["result"]["structuredContent"]["slug"], json!("blog"));
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("blog"), "{text}");

    // By name, as a person or a model would refer to it.
    let body = s
        .tool(&token, "get_compose", json!({ "stack": "blog" }))
        .await;
    assert_eq!(body["result"]["isError"], json!(false), "{body}");
    assert!(
        body["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("nginx:alpine")
    );

    // Recorded under the token's name, like any API call it makes.
    let (_, audit) = s.api("GET", "/api/v1/audit", None).await;
    let entry = audit
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["action"] == json!("register stack"))
        .unwrap();
    assert!(
        entry["username"].as_str().unwrap().contains("(token mcp"),
        "{entry}"
    );
}

#[tokio::test]
async fn a_failing_tool_says_why_as_a_result_not_a_protocol_error() {
    // A model can read a result and try something else; a protocol error
    // reads as the server being broken.
    let s = setup().await;
    let token = s.token(&["host.view"]).await;
    let body = s
        .tool(
            &token,
            "get_stack",
            json!({ "stack": "nothing-by-this-name" }),
        )
        .await;
    assert_eq!(body["result"]["isError"], json!(true), "{body}");
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("nothing-by-this-name"), "{text}");

    // No daemon in these tests: the daemon's absence is reported the same way.
    let body = s.tool(&token, "list_stacks", json!({})).await;
    assert_eq!(body["result"]["isError"], json!(true), "{body}");

    let body = s.tool(&token, "get_stack", json!({})).await;
    assert_eq!(
        body["result"]["isError"],
        json!(true),
        "a missing argument: {body}"
    );
}

/// A git server that accepts connections and never answers, so a deploy
/// fetching from it stays running until the listener is dropped.
async fn silent_remote() -> (tokio::task::JoinHandle<()>, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("git://{}/stacks.git", listener.local_addr().unwrap());
    let held = tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            open.push(socket);
        }
    });
    (held, url)
}

impl Setup {
    /// A stack deployed from `url`, returning its id.
    async fn git_stack(&self, url: &str) -> i64 {
        let (status, repo) = self
            .api("POST", "/api/v1/repos", Some(json!({ "url": url })))
            .await;
        assert_eq!(status, StatusCode::OK, "{repo}");
        let (status, stack) = self
            .api(
                "POST",
                "/api/v1/hosts/1/stacks/git",
                Some(json!({
                    "name": "Busy", "repo_id": repo["id"],
                    "git_ref": "refs/heads/main", "compose_path": "compose.yaml"
                })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{stack}");
        stack["id"].as_i64().unwrap()
    }
}

#[tokio::test]
async fn a_deploy_is_reported_when_it_ends() {
    let s = setup().await;
    // Compose refuses this at once, so the deploy ends quickly, and failed.
    let (status, stack) = s
        .api(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(json!({ "name": "Broken", "compose_yaml": "services:\n  web: [\n" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{stack}");
    let token = s.token(&["host.view", "stacks.deploy"]).await;

    let started = std::time::Instant::now();
    let body = s
        .tool(
            &token,
            "deploy_stack",
            json!({ "stack": "broken", "timeout_seconds": 60 }),
        )
        .await;

    let result = &body["result"]["structuredContent"];
    assert_eq!(result["status"], json!("failed"), "{body}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "not the whole wait: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_deploy_still_running_when_the_wait_runs_out_says_so() {
    let (remote, url) = silent_remote().await;
    let s = setup().await;
    let id = s.git_stack(&url).await;
    let token = s.token(&["host.view", "stacks.deploy"]).await;

    let body = s
        .tool(
            &token,
            "deploy_stack",
            json!({ "stack": id.to_string(), "timeout_seconds": 1 }),
        )
        .await;
    remote.abort();

    let result = &body["result"]["structuredContent"];
    assert_eq!(result["status"], json!("running"), "{body}");
    assert!(
        result["note"]
            .as_str()
            .is_some_and(|n| n.contains("wait ran out")),
        "{body}"
    );
}

#[tokio::test]
async fn revoking_the_token_ends_a_wait_for_an_outcome() {
    // A wait can last fifteen minutes; it must not outlive the token that
    // started it by more than a moment.
    let (remote, url) = silent_remote().await;
    let s = setup().await;
    let id = s.git_stack(&url).await;
    let token = s.token(&["host.view", "stacks.deploy"]).await;
    let (_, list) = s.api("GET", "/api/v1/tokens", None).await;
    let token_id = list[0]["id"].as_i64().unwrap();

    let started = std::time::Instant::now();
    let revoke = async {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        s.api("DELETE", &format!("/api/v1/tokens/{token_id}"), None)
            .await;
    };
    let (body, ()) = tokio::join!(
        s.tool(
            &token,
            "deploy_stack",
            json!({ "stack": id.to_string(), "timeout_seconds": 120 }),
        ),
        revoke
    );
    remote.abort();

    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "waited {:?} after the token was revoked",
        started.elapsed()
    );
    let result = &body["result"]["structuredContent"];
    assert_eq!(result["status"], json!("running"), "{body}");
    assert!(
        result["note"]
            .as_str()
            .is_some_and(|n| n.contains("could not be followed")),
        "{body}"
    );
}

#[tokio::test]
async fn a_revoked_token_is_refused_on_its_next_call() {
    let s = setup().await;
    let token = s.token(&["host.view"]).await;
    s.rpc(&token, "ping", json!({})).await;

    let (_, list) = s.api("GET", "/api/v1/tokens", None).await;
    let id = list[0]["id"].as_i64().unwrap();
    s.api("DELETE", &format!("/api/v1/tokens/{id}"), None).await;

    let (status, _, _) = s
        .mcp(
            &token,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn figures_and_sizing_are_tools_for_host_view() {
    let s = setup().await;
    let token = s.token(&["host.view"]).await;
    let names = tool_names(&s.rpc(&token, "tools/list", json!({})).await);
    assert!(
        names.contains(&"get_metrics".to_owned())
            && names.contains(&"sizing_recommendations".to_owned())
    );
    let body = s
        .tool(
            &token,
            "get_metrics",
            json!({ "subject": "host", "range": "24h" }),
        )
        .await;
    assert_eq!(body["result"]["isError"], json!(false), "{body}");
    let bad = s
        .tool(
            &token,
            "get_metrics",
            json!({ "subject": "host", "range": "forever" }),
        )
        .await;
    assert_eq!(bad["result"]["isError"], json!(true));
    let sizing = s.tool(&token, "sizing_recommendations", json!({})).await;
    assert_eq!(sizing["result"]["isError"], json!(false), "{sizing}");
}

#[tokio::test]
async fn a_metrics_subject_cannot_carry_its_own_query() {
    let s = setup().await;
    let token = s.token(&["host.view"]).await;
    for subject in [
        "web-1&extra=1",
        "host&range=1y",
        "web-1?x=1",
        "stack:blog#x",
        "../../x",
    ] {
        let body = s
            .tool(
                &token,
                "get_metrics",
                json!({ "subject": subject, "range": "24h" }),
            )
            .await;
        assert_eq!(body["result"]["isError"], json!(true), "{subject}: {body}");
    }
}
