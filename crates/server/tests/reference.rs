//! The reference against the server it describes.
//!
//! The endpoint table is what `GET /api/v1/reference` serves, so a route
//! missing from it is undocumented and an entry naming the wrong permission
//! misleads every client written from it. These tests hold the two together:
//!
//! - every documented endpoint is mounted;
//! - every route in the source is documented;
//! - every documented access rule is the one the handler enforces.

use std::collections::HashMap;
use std::path::Path;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use server::app;
use server::state::AppState;
use shared::reference::{Access, Endpoint};
use shared::token::Permission;
use store::Store;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";

/// Stands in for every path parameter. No record has this id, so a handler
/// that gets past its permission check answers 404 for the record, and
/// nothing is changed, run or removed.
const ABSENT: &str = "999999";

enum Caller<'a> {
    Anonymous,
    Session(&'a str),
    Token(&'a str),
}

struct Server {
    router: Router,
    cookie: String,
    /// Token secrets by name, issued once each.
    tokens: HashMap<String, String>,
    _stacks_root: tempfile::TempDir,
}

impl Server {
    /// A fresh instance with an administrator signed in. No Docker, as in
    /// the API tests: nothing here may reach a daemon.
    async fn start() -> Self {
        let store = Store::open_in_memory().await.expect("store");
        let stacks_root = tempfile::tempdir().expect("temp dir");
        let router = app::build(AppState::new(store, None, stacks_root.path()), false, None);
        let req = Request::post("/api/v1/auth/bootstrap")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "username": "admin", "password": PASSWORD }).to_string(),
            ))
            .expect("request");
        let res = router.clone().oneshot(req).await.expect("response");
        assert_eq!(res.status(), StatusCode::OK);
        let cookie = res
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .expect("a session cookie")
            .to_owned();
        Self {
            router,
            cookie,
            tokens: HashMap::new(),
            _stacks_root: stacks_root,
        }
    }

    /// Sends `method path` with no body.
    ///
    /// The body is read only for an error status: a successful stream (the
    /// event stream, say) never ends, and its status is all that matters.
    async fn send(&self, method: &str, path: &str, caller: &Caller<'_>) -> (StatusCode, Bytes) {
        let mut req = Request::builder().method(method).uri(path);
        match caller {
            Caller::Anonymous => {}
            Caller::Session(cookie) => req = req.header("cookie", *cookie),
            Caller::Token(secret) => {
                req = req.header("authorization", format!("Bearer {secret}"));
            }
        }
        let res = self
            .router
            .clone()
            .oneshot(req.body(Body::empty()).expect("request"))
            .await
            .expect("response");
        let status = res.status();
        if status.is_success() || status.is_redirection() {
            return (status, Bytes::new());
        }
        let bytes = res.into_body().collect().await.expect("body").to_bytes();
        (status, bytes)
    }

    /// The secret of a token granted exactly `permissions`, issued on first
    /// use.
    async fn token(&mut self, name: &str, permissions: &[Permission]) -> String {
        if let Some(secret) = self.tokens.get(name) {
            return secret.clone();
        }
        let req = Request::post("/api/v1/tokens")
            .header("content-type", "application/json")
            .header("cookie", &self.cookie)
            .body(Body::from(
                json!({ "name": name, "permissions": permissions, "expires_in_days": null })
                    .to_string(),
            ))
            .expect("request");
        let res = self.router.clone().oneshot(req).await.expect("response");
        assert_eq!(res.status(), StatusCode::OK, "issuing {name}");
        let bytes = res.into_body().collect().await.expect("body").to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        let secret = body["secret"].as_str().expect("secret").to_owned();
        self.tokens.insert(name.to_owned(), secret.clone());
        secret
    }
}

/// `path` with every `{param}` replaced by [`ABSENT`].
fn concrete(path: &str) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}').expect("a closed parameter") + open;
        out.push_str(ABSENT);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Whether axum's router answered rather than a handler: it had no route
/// for the path (404) or none for the method on it (405).
///
/// A handler can answer 404 too, for a record that does not exist, which
/// every probe here asks for. The two are told apart by the body. The
/// router's answers are empty; every refusal from a GhostDock handler or
/// extractor goes through `ApiError`, which always carries a JSON body with
/// a `code`. `looks_like_the_router` below checks this reading against
/// requests that really do miss.
fn router_miss(status: StatusCode, body: &[u8]) -> bool {
    matches!(
        status,
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
    ) && serde_json::from_slice::<shared::ApiError>(body).is_err()
}

fn code(body: &[u8]) -> String {
    serde_json::from_slice::<shared::ApiError>(body)
        .map(|e| e.code)
        .unwrap_or_default()
}

fn label(e: &Endpoint) -> String {
    format!("{} {}", e.method, e.path)
}

#[tokio::test]
async fn looks_like_the_router() {
    let server = Server::start().await;
    let session = Caller::Session(&server.cookie);

    let (status, body) = server.send("GET", "/api/v1/nowhere", &session).await;
    assert!(router_miss(status, &body), "an unmounted path: {status}");
    let (status, body) = server.send("DELETE", "/api/v1/health", &session).await;
    assert!(router_miss(status, &body), "an unmounted method: {status}");

    // A handler's own 404 is not mistaken for the router's.
    let (status, body) = server.send("GET", "/api/v1/stacks/999999", &session).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!router_miss(status, &body));
}

#[tokio::test]
async fn the_table_has_each_endpoint_once() {
    let endpoints = server::reference::endpoints();
    assert!(!endpoints.is_empty());
    let mut seen = std::collections::HashSet::new();
    for e in &endpoints {
        assert!(seen.insert(label(e)), "{} is listed twice", label(e));
        assert!(!e.summary.trim().is_empty(), "{} has no summary", label(e));
        assert!(!e.area.trim().is_empty(), "{} has no area", label(e));
        assert!(
            ["GET", "POST", "PUT", "DELETE"].contains(&e.method.as_str()),
            "{}",
            label(e)
        );
    }
}

/// Every documented endpoint is mounted. Asked anonymously, so nothing
/// behind a permission does any work; a public one gets no body.
#[tokio::test]
async fn every_documented_endpoint_is_mounted() {
    let server = Server::start().await;
    for e in server::reference::endpoints() {
        let (status, body) = server
            .send(&e.method, &concrete(&e.path), &Caller::Anonymous)
            .await;
        assert!(
            !router_miss(status, &body),
            "{} is documented but not mounted ({status})",
            label(&e)
        );
    }
}

/// Every `.route("…")` in the server's source is in the table.
///
/// Routes are normally declared through the table itself (see
/// `server::reference::Routes`), which mounts what it documents. This catches
/// a route added beside it with a plain `.route(…)`: that one must be listed
/// by hand, and here is where forgetting to is noticed. A `.route(` whose
/// path is not a literal cannot be checked, so it is allowed only in the
/// table's own code.
#[test]
fn every_route_in_the_source_is_documented() {
    let documented: Vec<String> = server::reference::endpoints()
        .into_iter()
        .map(|e| e.path)
        .collect();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(!files.is_empty());

    let mut literals = 0;
    for file in files {
        let text = std::fs::read_to_string(&file).expect("source");
        let name = file.strip_prefix(&src).expect("under src").display();
        for (line, path) in route_literals(&text) {
            match path {
                Some(path) => {
                    literals += 1;
                    let nested = format!("{}{path}", server::reference::API_PREFIX);
                    assert!(
                        documented.contains(&path) || documented.contains(&nested),
                        "{name}:{line} mounts {path}, which the reference does not list"
                    );
                }
                None => assert_eq!(
                    name.to_string(),
                    "reference.rs",
                    "{name}:{line} calls .route( with a path that is not a literal"
                ),
            }
        }
    }
    // The scan itself is under test: /mcp is mounted this way.
    assert!(literals > 0, "no .route(\"…\") found; is the scan broken?");
}

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

/// Each `.route(` call: its line, and its path when that is a string literal.
fn route_literals(text: &str) -> Vec<(usize, Option<String>)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(".route(") {
        let start = from + at;
        let line = text[..start].lines().count().max(1);
        let args = text[start + ".route(".len()..].trim_start();
        let path = args
            .strip_prefix('"')
            .and_then(|rest| rest.split_once('"'))
            .map(|(path, _)| path.to_owned());
        found.push((line, path));
        from = start + 1;
    }
    found
}

#[test]
fn the_scan_finds_split_and_single_line_routes() {
    let text = "Router::new()\n    .route(\"/a\", get(x))\n    .route(\n        \"/b/{id}\",\n        post(y),\n    )\n    .route(path, z)\n";
    assert_eq!(
        route_literals(text),
        vec![
            (2, Some("/a".to_owned())),
            (3, Some("/b/{id}".to_owned())),
            (7, None),
        ]
    );
}

/// What each entry says about access is what its handler enforces.
#[tokio::test]
async fn every_documented_access_rule_is_enforced() {
    let mut server = Server::start().await;
    let cookie = server.cookie.clone();
    let all = server.token("all", &Permission::ALL).await;

    for e in server::reference::endpoints() {
        let path = concrete(&e.path);
        let what = label(&e);
        match e.access {
            Access::Public => {
                let (status, _) = server.send(&e.method, &path, &Caller::Anonymous).await;
                assert_ne!(
                    status,
                    StatusCode::UNAUTHORIZED,
                    "{what} is documented public"
                );
            }
            Access::Session => {
                let (status, body) = server.send(&e.method, &path, &Caller::Token(&all)).await;
                assert_eq!(
                    (status, code(&body).as_str()),
                    (StatusCode::FORBIDDEN, "session_only"),
                    "{what} is documented session-only, yet a token was not refused"
                );
                let (status, _) = server.send(&e.method, &path, &Caller::Anonymous).await;
                assert_eq!(status, StatusCode::UNAUTHORIZED, "{what} anonymously");
                let (status, _) = server
                    .send(&e.method, &path, &Caller::Session(&cookie))
                    .await;
                assert!(
                    refused(status).is_none(),
                    "{what} refused a session: {status}"
                );
            }
            Access::Authenticated => {
                let one = server.token("one", &[Permission::ActivityView]).await;
                for caller in [Caller::Token(&one), Caller::Session(&cookie)] {
                    let (status, _) = server.send(&e.method, &path, &caller).await;
                    assert!(refused(status).is_none(), "{what} refused: {status}");
                }
                let (status, _) = server.send(&e.method, &path, &Caller::Anonymous).await;
                assert_eq!(status, StatusCode::UNAUTHORIZED, "{what} anonymously");
            }
            Access::Token(p) => {
                let others: Vec<Permission> =
                    Permission::ALL.into_iter().filter(|q| *q != p).collect();
                let without = server
                    .token(&format!("without {}", p.as_str()), &others)
                    .await;
                let only = server.token(&format!("only {}", p.as_str()), &[p]).await;

                let (status, body) = server
                    .send(&e.method, &path, &Caller::Token(&without))
                    .await;
                assert_eq!(
                    (status, code(&body).as_str()),
                    (StatusCode::FORBIDDEN, "missing_permission"),
                    "{what} is documented as needing {}, yet a token without it was not refused",
                    p.as_str()
                );
                let (status, _) = server.send(&e.method, &path, &Caller::Token(&only)).await;
                assert!(
                    refused(status).is_none(),
                    "{what} refused a token granted only {}: {status}",
                    p.as_str()
                );
                let (status, _) = server
                    .send(&e.method, &path, &Caller::Session(&cookie))
                    .await;
                assert!(
                    refused(status).is_none(),
                    "{what} refused a session: {status}"
                );
                let (status, _) = server.send(&e.method, &path, &Caller::Anonymous).await;
                assert_eq!(status, StatusCode::UNAUTHORIZED, "{what} anonymously");
            }
            Access::AnyToken => {
                let one = server.token("one", &[Permission::ActivityView]).await;
                let (status, _) = server.send(&e.method, &path, &Caller::Token(&one)).await;
                assert!(
                    refused(status).is_none(),
                    "{what} refused a token: {status}"
                );
                let (status, _) = server
                    .send(&e.method, &path, &Caller::Session(&cookie))
                    .await;
                assert!(
                    refused(status).is_some(),
                    "{what} admitted a session: {status}"
                );
            }
        }
    }
}

/// The refusal a status is, if it is one: 401 or 403.
fn refused(status: StatusCode) -> Option<StatusCode> {
    matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN).then_some(status)
}

/// Every permission a token can hold names at least one endpoint or tool, so
/// none is granted for nothing, and every tool's permission is one of them.
#[test]
fn the_reference_lists_every_permission_and_tool() {
    let reference = server::reference::reference();
    let listed: Vec<Permission> = reference.permissions.iter().map(|p| p.permission).collect();
    assert_eq!(listed, Permission::ALL.to_vec());
    assert!(!reference.tools.is_empty());
    for tool in &reference.tools {
        let schema: serde_json::Value =
            serde_json::from_str(&tool.input_schema).expect("the schema is JSON");
        assert_eq!(schema["type"], json!("object"), "{}", tool.name);
    }
    for p in Permission::ALL {
        let used = reference
            .endpoints
            .iter()
            .any(|e| e.access.permission() == Some(p))
            || reference.tools.iter().any(|t| t.permission == p);
        assert!(used, "{} guards nothing", p.as_str());
    }
}
