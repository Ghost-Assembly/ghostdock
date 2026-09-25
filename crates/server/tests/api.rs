//! End-to-end API behaviour, exercised through the real router.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use server::app;
use server::state::AppState;
use store::Store;
use tower::ServiceExt;

/// A client that remembers its session cookie, so multi-step flows
/// (bootstrap, then call a protected route) behave like a real browser.
struct Client {
    router: Router,
    cookie: Option<String>,
    bearer: Option<String>,
    // Held so the stacks directory outlives the test.
    _stacks_root: tempfile::TempDir,
}

impl Client {
    async fn new() -> Self {
        let store = Store::open_in_memory().await.expect("store");
        let stacks_root = tempfile::tempdir().expect("temp dir");
        // No Docker in tests: daemon-backed routes must degrade to 503
        // rather than panic, and that is itself worth asserting.
        Self {
            router: app::build(AppState::new(store, None, stacks_root.path()), false, None),
            cookie: None,
            bearer: None,
            _stacks_root: stacks_root,
        }
    }

    async fn send(&mut self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(cookie) = &self.cookie {
            req = req.header("cookie", cookie);
        }
        if let Some(token) = &self.bearer {
            req = req.header("authorization", format!("Bearer {token}"));
        }
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .expect("request");

        let res = self.router.clone().oneshot(req).await.expect("response");
        let status = res.status();

        if let Some(set) = res.headers().get("set-cookie") {
            let raw = set.to_str().expect("cookie is text");
            self.cookie = Some(raw.split(';').next().unwrap_or(raw).to_owned());
        }

        let bytes = res.into_body().collect().await.expect("body").to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    /// A second browser against the same instance: same server, own cookie.
    fn sibling(&self) -> Self {
        Self {
            router: self.router.clone(),
            cookie: None,
            bearer: None,
            _stacks_root: tempfile::tempdir().expect("temp dir"),
        }
    }

    fn credentials(user: &str, password: &str) -> Value {
        json!({ "username": user, "password": password })
    }
}

const PASSWORD: &str = "correct horse battery staple";

#[tokio::test]
async fn health_is_public() {
    let mut c = Client::new().await;
    let (status, _) = c.send("GET", "/api/v1/health", None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_fresh_instance_reports_it_needs_bootstrapping() {
    let mut c = Client::new().await;
    let (status, body) = c.send("GET", "/api/v1/auth/status", None).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["bootstrapped"], json!(false));
    assert_eq!(body["user"], Value::Null);
}

#[tokio::test]
async fn protected_routes_reject_anonymous_callers() {
    let mut c = Client::new().await;
    for uri in [
        "/api/v1/hosts",
        "/api/v1/hosts/1",
        "/api/v1/hosts/1/containers",
        "/api/v1/hosts/1/stacks",
    ] {
        let (status, body) = c.send("GET", uri, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri} must require auth");
        assert_eq!(body["code"], json!("not_authenticated"));
    }
}

#[tokio::test]
async fn bootstrap_creates_the_admin_and_logs_them_in() {
    let mut c = Client::new().await;
    let (status, body) = c
        .send(
            "POST",
            "/api/v1/auth/bootstrap",
            Some(Client::credentials("admin", PASSWORD)),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["username"], json!("admin"));
    assert!(
        body.get("password_hash").is_none(),
        "no password material on the wire"
    );

    let (status, body) = c.send("GET", "/api/v1/hosts", None).await;
    assert_eq!(status, StatusCode::OK, "bootstrap must establish a session");
    assert_eq!(body[0]["name"], json!("local"));
}

#[tokio::test]
async fn bootstrap_is_refused_once_an_admin_exists() {
    let mut c = Client::new().await;
    c.send(
        "POST",
        "/api/v1/auth/bootstrap",
        Some(Client::credentials("admin", PASSWORD)),
    )
    .await;

    let (status, body) = c
        .send(
            "POST",
            "/api/v1/auth/bootstrap",
            Some(Client::credentials("sneaky", PASSWORD)),
        )
        .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], json!("conflict"));
}

#[tokio::test]
async fn bootstrap_enforces_the_password_policy() {
    let mut c = Client::new().await;
    let (status, body) = c
        .send(
            "POST",
            "/api/v1/auth/bootstrap",
            Some(Client::credentials("admin", "short")),
        )
        .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], json!("bad_request"));

    let (status, _) = c.send("GET", "/api/v1/auth/status", None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = c.send("GET", "/api/v1/auth/status", None).await;
    assert_eq!(body["bootstrapped"], json!(false), "no account was created");
}

#[tokio::test]
async fn login_succeeds_and_logout_revokes_access() {
    let mut c = Client::new().await;
    c.send(
        "POST",
        "/api/v1/auth/bootstrap",
        Some(Client::credentials("admin", PASSWORD)),
    )
    .await;

    let (status, _) = c.send("POST", "/api/v1/auth/logout", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = c.send("GET", "/api/v1/hosts", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the session is gone");

    let (status, body) = c
        .send(
            "POST",
            "/api/v1/auth/login",
            Some(Client::credentials("admin", PASSWORD)),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["username"], json!("admin"));

    let (status, _) = c.send("GET", "/api/v1/hosts", None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_wrong_password_and_an_unknown_user_are_indistinguishable() {
    let mut c = Client::new().await;
    c.send(
        "POST",
        "/api/v1/auth/bootstrap",
        Some(Client::credentials("admin", PASSWORD)),
    )
    .await;
    c.send("POST", "/api/v1/auth/logout", None).await;

    let (wrong_status, wrong_body) = c
        .send(
            "POST",
            "/api/v1/auth/login",
            Some(Client::credentials("admin", "wrong password here")),
        )
        .await;
    let (missing_status, missing_body) = c
        .send(
            "POST",
            "/api/v1/auth/login",
            Some(Client::credentials("ghost", "wrong password here")),
        )
        .await;

    assert_eq!(wrong_status, StatusCode::UNAUTHORIZED);
    assert_eq!(missing_status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        wrong_body, missing_body,
        "responses must not reveal whether an account exists"
    );
}

#[tokio::test]
async fn logging_in_issues_a_new_session_id() {
    let mut c = Client::new().await;
    c.send(
        "POST",
        "/api/v1/auth/bootstrap",
        Some(Client::credentials("admin", PASSWORD)),
    )
    .await;
    c.send("POST", "/api/v1/auth/logout", None).await;
    let before = c.cookie.clone();

    c.send(
        "POST",
        "/api/v1/auth/login",
        Some(Client::credentials("admin", PASSWORD)),
    )
    .await;

    assert_ne!(
        before, c.cookie,
        "a session id must be reissued at login, or a planted cookie survives it"
    );
}

#[tokio::test]
async fn an_unknown_host_is_not_found_even_when_authenticated() {
    let mut c = Client::new().await;
    c.send(
        "POST",
        "/api/v1/auth/bootstrap",
        Some(Client::credentials("admin", PASSWORD)),
    )
    .await;

    let (status, body) = c.send("GET", "/api/v1/hosts/999/containers", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], json!("not_found"));
}

#[tokio::test]
async fn daemon_backed_routes_degrade_to_503_when_docker_is_absent() {
    let mut c = Client::new().await;
    c.send(
        "POST",
        "/api/v1/auth/bootstrap",
        Some(Client::credentials("admin", PASSWORD)),
    )
    .await;

    let (status, body) = c.send("GET", "/api/v1/hosts/1/containers", None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], json!("daemon_unavailable"));
}

// ---- stack management -------------------------------------------------

const COMPOSE: &str = "services:\n  web:\n    image: nginx:alpine\n";

impl Client {
    async fn signed_in() -> Self {
        let mut c = Self::new().await;
        c.send(
            "POST",
            "/api/v1/auth/bootstrap",
            Some(Self::credentials("admin", PASSWORD)),
        )
        .await;
        c
    }

    fn stack(name: &str, yaml: &str) -> Value {
        json!({ "name": name, "compose_yaml": yaml })
    }
}

#[tokio::test]
async fn registering_a_stack_derives_a_project_name_from_it() {
    let mut c = Client::signed_in().await;

    let (status, body) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(Client::stack("My Blog", COMPOSE)),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], json!("My Blog"));
    assert_eq!(
        body["slug"],
        json!("my-blog"),
        "the compose project name is derived, not taken verbatim"
    );
    assert_eq!(body["source_kind"], json!("inline"));
}

#[tokio::test]
async fn a_stack_needs_a_usable_name_and_a_compose_file() {
    let mut c = Client::signed_in().await;

    for (label, body) in [
        ("empty name", Client::stack("   ", COMPOSE)),
        ("empty compose", Client::stack("Blog", "  \n")),
        (
            "name with nothing to build an identifier from",
            Client::stack("!!!", COMPOSE),
        ),
    ] {
        let (status, _) = c.send("POST", "/api/v1/hosts/1/stacks", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label} must be refused");
    }
}

#[tokio::test]
async fn two_stacks_cannot_share_a_project_name() {
    let mut c = Client::signed_in().await;
    c.send(
        "POST",
        "/api/v1/hosts/1/stacks",
        Some(Client::stack("Blog", COMPOSE)),
    )
    .await;

    // Different display name, same derived project name.
    let (status, body) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(Client::stack("blog", COMPOSE)),
        )
        .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body["message"].as_str().unwrap().contains("blog"),
        "the message should name the clash: {body}"
    );
}

#[tokio::test]
async fn a_stack_can_be_read_updated_and_forgotten() {
    let mut c = Client::signed_in().await;
    let (_, created) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(Client::stack("Blog", COMPOSE)),
        )
        .await;
    let id = created["id"].as_i64().unwrap();

    let (status, body) = c.send("GET", &format!("/api/v1/stacks/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["slug"], json!("blog"));

    let (status, _) = c
        .send(
            "PUT",
            &format!("/api/v1/stacks/{id}"),
            Some(Client::stack("Blog", "services: {}\n")),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = c
        .send("DELETE", &format!("/api/v1/stacks/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = c.send("GET", &format!("/api/v1/stacks/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn operations_on_an_unknown_stack_are_not_found() {
    let mut c = Client::signed_in().await;

    for (method, uri) in [
        ("GET", "/api/v1/stacks/999"),
        ("POST", "/api/v1/stacks/999/deploy"),
        ("POST", "/api/v1/stacks/999/stop"),
        ("POST", "/api/v1/stacks/999/restart"),
        ("POST", "/api/v1/stacks/999/down"),
        ("GET", "/api/v1/stacks/999/deployments"),
        ("GET", "/api/v1/deployments/999"),
    ] {
        let (status, _) = c.send(method, uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
    }
}

#[tokio::test]
async fn stack_management_requires_authentication() {
    let mut c = Client::new().await;
    // Bootstrap so the instance is set up, then sign out.
    c.send(
        "POST",
        "/api/v1/auth/bootstrap",
        Some(Client::credentials("admin", PASSWORD)),
    )
    .await;
    c.send("POST", "/api/v1/auth/logout", None).await;

    for (method, uri) in [
        ("POST", "/api/v1/hosts/1/stacks"),
        ("GET", "/api/v1/stacks/1"),
        ("POST", "/api/v1/stacks/1/deploy"),
        ("POST", "/api/v1/stacks/1/down"),
        ("GET", "/api/v1/events"),
    ] {
        let (status, _) = c.send(method, uri, Some(json!({}))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

#[tokio::test]
async fn a_new_stack_has_no_history_yet() {
    let mut c = Client::signed_in().await;
    let (_, created) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(Client::stack("Blog", COMPOSE)),
        )
        .await;
    let id = created["id"].as_i64().unwrap();

    let (status, body) = c
        .send("GET", &format!("/api/v1/stacks/{id}/deployments"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]));
}

// ---- git sources, credentials and environments ------------------------

const TOKEN: &str = "ghp_a_real_looking_token";

impl Client {
    async fn credential(&mut self, name: &str) -> i64 {
        let (_, body) = self
            .send(
                "POST",
                "/api/v1/credentials",
                Some(json!({ "name": name, "username": "x-access-token", "secret": TOKEN })),
            )
            .await;
        body["id"].as_i64().expect("credential id")
    }

    async fn repo(&mut self, url: &str, credential_id: Option<i64>) -> i64 {
        let (_, body) = self
            .send(
                "POST",
                "/api/v1/repos",
                Some(json!({ "url": url, "credential_id": credential_id })),
            )
            .await;
        body["id"].as_i64().expect("repo id")
    }
}

#[tokio::test]
async fn a_credential_secret_never_comes_back_out() {
    let mut c = Client::signed_in().await;
    let (status, created) = c
        .send(
            "POST",
            "/api/v1/credentials",
            Some(json!({ "name": "github", "username": "x", "secret": TOKEN })),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        !created.to_string().contains(TOKEN),
        "the secret came back in the create response: {created}"
    );

    let (_, listed) = c.send("GET", "/api/v1/credentials", None).await;
    assert!(
        !listed.to_string().contains(TOKEN),
        "the secret came back in the listing: {listed}"
    );
    assert_eq!(listed[0]["name"], json!("github"));
}

#[tokio::test]
async fn a_credential_needs_a_name_and_a_secret() {
    let mut c = Client::signed_in().await;
    for body in [
        json!({ "name": "  ", "username": "x", "secret": TOKEN }),
        json!({ "name": "github", "username": "x", "secret": "" }),
    ] {
        let (status, _) = c.send("POST", "/api/v1/credentials", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn credential_names_are_unique() {
    let mut c = Client::signed_in().await;
    c.credential("github").await;
    let (status, _) = c
        .send(
            "POST",
            "/api/v1/credentials",
            Some(json!({ "name": "github", "username": "x", "secret": TOKEN })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_repository_reports_its_credential_by_name() {
    let mut c = Client::signed_in().await;
    let cred = c.credential("github").await;
    let (status, repo) = c
        .send(
            "POST",
            "/api/v1/repos",
            Some(json!({ "url": "https://example.invalid/s.git", "credential_id": cred })),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(repo["credential_name"], json!("github"));
}

#[tokio::test]
async fn a_repository_cannot_reference_a_credential_that_is_gone() {
    let mut c = Client::signed_in().await;
    let (status, _) = c
        .send(
            "POST",
            "/api/v1/repos",
            Some(json!({ "url": "https://example.invalid/s.git", "credential_id": 999 })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_repository_with_stacks_in_it_cannot_be_removed() {
    let mut c = Client::signed_in().await;
    let repo = c.repo("https://example.invalid/s.git", None).await;
    c.send(
        "POST",
        "/api/v1/hosts/1/stacks/git",
        Some(json!({
            "name": "Blog", "repo_id": repo,
            "git_ref": "refs/heads/main", "compose_path": "compose/blog.yml"
        })),
    )
    .await;

    let (status, body) = c
        .send("DELETE", &format!("/api/v1/repos/{repo}"), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("Remove them first"),
        "the message should say what to do: {body}"
    );
}

#[tokio::test]
async fn registering_a_git_stack_records_where_its_file_lives() {
    let mut c = Client::signed_in().await;
    let repo = c.repo("https://example.invalid/s.git", None).await;

    let (status, stack) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks/git",
            Some(json!({
                "name": "My Blog", "repo_id": repo,
                "git_ref": "refs/heads/main", "compose_path": "compose/blog.yml"
            })),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(stack["slug"], json!("my-blog"));
    assert_eq!(stack["source_kind"], json!("git"));
    assert_eq!(
        stack["git"]["repo_url"],
        json!("https://example.invalid/s.git")
    );
    assert_eq!(stack["git"]["compose_path"], json!("compose/blog.yml"));
    assert_eq!(stack["git"]["last_commit"], Value::Null);
}

#[tokio::test]
async fn a_compose_path_that_escapes_the_repository_is_refused_at_registration() {
    // Caught where the person who typed it can fix it, not at deploy time.
    let mut c = Client::signed_in().await;
    let repo = c.repo("https://example.invalid/s.git", None).await;

    for path in [
        "../../../etc/passwd",
        "/etc/passwd",
        "compose/../../out.yml",
        "..",
    ] {
        let (status, _) = c
            .send(
                "POST",
                "/api/v1/hosts/1/stacks/git",
                Some(json!({
                    "name": format!("s{}", path.len()), "repo_id": repo,
                    "git_ref": "refs/heads/main", "compose_path": path
                })),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path:?} must be refused");
    }
}

#[tokio::test]
async fn a_git_stack_needs_a_ref_and_a_real_repository() {
    let mut c = Client::signed_in().await;
    let repo = c.repo("https://example.invalid/s.git", None).await;

    let (status, _) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks/git",
            Some(json!({ "name": "A", "repo_id": repo, "git_ref": " ", "compose_path": "a.yml" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks/git",
            Some(json!({
                "name": "B", "repo_id": 999,
                "git_ref": "refs/heads/main", "compose_path": "a.yml"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_repository_url_git_would_misread_is_refused() {
    // Each of these makes git run a command or reach a transport GhostDock
    // never offered. None may be stored.
    let mut c = Client::signed_in().await;
    for url in [
        "--upload-pack=touch /tmp/ghostdock-pwned",
        "ext::sh -c touch% /tmp/ghostdock-pwned",
        "ext::sh",
        "fd::3",
        "ftp://example.invalid/r.git",
        "/srv/git/local-path.git",
        "https://example.invalid/r .git",
    ] {
        let (status, body) = c
            .send(
                "POST",
                "/api/v1/repos",
                Some(json!({ "url": url, "credential_id": null })),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{url} was accepted");
        assert_eq!(body["code"], json!("bad_request"));
    }
    let (_, repos) = c.send("GET", "/api/v1/repos", None).await;
    assert_eq!(repos, json!([]), "nothing was stored");
}

#[tokio::test]
async fn a_password_in_a_repository_url_is_refused_with_where_it_belongs() {
    let mut c = Client::signed_in().await;
    let (status, body) = c
        .send(
            "POST",
            "/api/v1/repos",
            Some(
                json!({ "url": "https://me:hunter2@example.invalid/r.git", "credential_id": null }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let message = body["message"].as_str().unwrap();
    assert!(message.contains("Credentials"), "{message}");
    assert!(!message.contains("hunter2"), "the password was echoed back");
}

#[tokio::test]
async fn a_ref_git_would_read_as_an_option_is_refused() {
    let (_dir, url) = discovery_repo();
    let mut c = Client::signed_in().await;
    let repo = c.repo(&url, None).await;
    let bad = "--upload-pack=touch /tmp/ghostdock-pwned";

    let (status, _) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks/git",
            Some(json!({
                "name": "A", "repo_id": repo, "git_ref": bad, "compose_path": "a.yml"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "registering");

    for route in ["discover", "import"] {
        let (status, body) = c
            .send(
                "POST",
                &format!("/api/v1/repos/{repo}/{route}"),
                Some(json!({ "git_ref": bad, "pattern": null, "paths": ["compose.yaml"] })),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{route}: {body}");
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .contains("cannot start with -"),
            "{route}: {body}"
        );
    }
}

#[tokio::test]
async fn an_environment_can_be_set_and_its_names_listed_but_never_its_values() {
    let mut c = Client::signed_in().await;
    let (_, stack) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(Client::stack("App", COMPOSE)),
        )
        .await;
    let id = stack["id"].as_i64().unwrap();

    let (status, body) = c
        .send(
            "PUT",
            &format!("/api/v1/stacks/{id}/env"),
            Some(json!({ "vars": [
                { "key": "API_KEY", "value": "sk_live_secret" },
                { "key": "TZ", "value": "Europe/London" }
            ]})),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["keys"], json!(["API_KEY", "TZ"]));
    assert!(
        !body.to_string().contains("sk_live_secret"),
        "values must not come back: {body}"
    );

    let (_, listed) = c
        .send("GET", &format!("/api/v1/stacks/{id}/env"), None)
        .await;
    assert_eq!(listed["keys"], json!(["API_KEY", "TZ"]));
    assert!(!listed.to_string().contains("sk_live_secret"));
}

#[tokio::test]
async fn an_environment_value_that_could_define_another_variable_is_refused() {
    let mut c = Client::signed_in().await;
    let (_, stack) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(Client::stack("App", COMPOSE)),
        )
        .await;
    let id = stack["id"].as_i64().unwrap();

    let (status, body) = c
        .send(
            "PUT",
            &format!("/api/v1/stacks/{id}/env"),
            Some(json!({ "vars": [{ "key": "X", "value": "a\nADMIN=true" }]})),
        )
        .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("line break"));
}

#[tokio::test]
async fn source_management_requires_authentication() {
    let mut c = Client::new().await;
    c.send(
        "POST",
        "/api/v1/auth/bootstrap",
        Some(Client::credentials("admin", PASSWORD)),
    )
    .await;
    c.send("POST", "/api/v1/auth/logout", None).await;

    for (method, uri) in [
        ("GET", "/api/v1/credentials"),
        ("POST", "/api/v1/credentials"),
        ("GET", "/api/v1/repos"),
        ("POST", "/api/v1/hosts/1/stacks/git"),
        ("GET", "/api/v1/stacks/1/env"),
    ] {
        let (status, _) = c.send(method, uri, Some(json!({}))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

#[tokio::test]
async fn one_variable_can_be_changed_without_retyping_the_others() {
    // Values are write-only, so a wholesale replace would mean re-entering
    // every secret in order to change one.
    let mut c = Client::signed_in().await;
    let (_, stack) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(Client::stack("App", COMPOSE)),
        )
        .await;
    let id = stack["id"].as_i64().unwrap();

    c.send(
        "PUT",
        &format!("/api/v1/stacks/{id}/env"),
        Some(json!({ "vars": [
            { "key": "KEEP", "value": "untouched" },
            { "key": "CHANGE", "value": "before" }
        ]})),
    )
    .await;

    let (status, body) = c
        .send(
            "PUT",
            &format!("/api/v1/stacks/{id}/env/CHANGE"),
            Some(json!({ "value": "after" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["keys"],
        json!(["CHANGE", "KEEP"]),
        "the other survives"
    );

    let (status, body) = c
        .send("DELETE", &format!("/api/v1/stacks/{id}/env/CHANGE"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["keys"], json!(["KEEP"]));
}

#[tokio::test]
async fn a_single_variable_is_validated_too() {
    let mut c = Client::signed_in().await;
    let (_, stack) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(Client::stack("App", COMPOSE)),
        )
        .await;
    let id = stack["id"].as_i64().unwrap();

    let (status, _) = c
        .send(
            "PUT",
            &format!("/api/v1/stacks/{id}/env/X"),
            Some(json!({ "value": "a\nADMIN=true" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---- updates ----------------------------------------------------------

#[tokio::test]
async fn the_updates_view_lists_every_stack_even_when_nothing_is_waiting() {
    // Hiding current stacks would leave no way to tell "nothing waiting"
    // from "nothing checked".
    let mut c = Client::signed_in().await;
    c.send(
        "POST",
        "/api/v1/hosts/1/stacks",
        Some(Client::stack("App", COMPOSE)),
    )
    .await;

    let (status, body) = c.send("GET", "/api/v1/hosts/1/updates", None).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["stack"]["slug"], json!("app"));
    assert_eq!(body[0]["reason"], Value::Null, "nothing waiting yet");
    assert_eq!(body[0]["auto_apply"], json!(false));
    assert_eq!(
        body[0]["status"]["checked_at"],
        Value::Null,
        "never checked reads as unknown, not as current"
    );
}

#[tokio::test]
async fn auto_apply_can_be_turned_on_and_off() {
    let mut c = Client::signed_in().await;
    let (_, stack) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(Client::stack("App", COMPOSE)),
        )
        .await;
    let id = stack["id"].as_i64().unwrap();

    let (status, body) = c
        .send(
            "PUT",
            &format!("/api/v1/stacks/{id}/auto-apply"),
            Some(json!({ "enabled": true })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enabled"], json!(true));

    let (_, listed) = c.send("GET", "/api/v1/hosts/1/updates", None).await;
    assert_eq!(listed[0]["auto_apply"], json!(true));

    c.send(
        "PUT",
        &format!("/api/v1/stacks/{id}/auto-apply"),
        Some(json!({ "enabled": false })),
    )
    .await;
    let (_, listed) = c.send("GET", "/api/v1/hosts/1/updates", None).await;
    assert_eq!(listed[0]["auto_apply"], json!(false));
}

#[tokio::test]
async fn checking_an_unknown_stack_is_not_found() {
    let mut c = Client::signed_in().await;
    for (method, uri, body) in [
        ("POST", "/api/v1/stacks/999/check", None),
        (
            "PUT",
            "/api/v1/stacks/999/auto-apply",
            Some(json!({ "enabled": true })),
        ),
    ] {
        let (status, _) = c.send(method, uri, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
    }
}

#[tokio::test]
async fn update_routes_require_authentication() {
    let mut c = Client::new().await;
    c.send(
        "POST",
        "/api/v1/auth/bootstrap",
        Some(Client::credentials("admin", PASSWORD)),
    )
    .await;
    c.send("POST", "/api/v1/auth/logout", None).await;

    for (method, uri) in [
        ("GET", "/api/v1/hosts/1/updates"),
        ("POST", "/api/v1/stacks/1/check"),
        ("PUT", "/api/v1/stacks/1/auto-apply"),
    ] {
        let (status, _) = c.send(method, uri, Some(json!({ "enabled": true }))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

// ---- audit ------------------------------------------------------------

#[tokio::test]
async fn actions_are_recorded_with_who_did_them() {
    let mut c = Client::signed_in().await;
    let (_, stack) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(Client::stack("App", COMPOSE)),
        )
        .await;
    let id = stack["id"].as_i64().unwrap();
    c.send("DELETE", &format!("/api/v1/stacks/{id}"), None)
        .await;

    let (status, body) = c.send("GET", "/api/v1/audit", None).await;
    assert_eq!(status, StatusCode::OK);

    let actions: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert!(actions.contains(&"forget stack"), "got {actions:?}");
    assert!(actions.contains(&"register stack"), "got {actions:?}");
    assert!(actions.contains(&"bootstrap"), "got {actions:?}");
    assert_eq!(body[0]["username"], json!("admin"));
    assert_eq!(body[0]["target"], json!("app"), "newest first");
}

#[tokio::test]
async fn a_failed_sign_in_is_recorded() {
    // A run of these is precisely what an audit trail exists to make visible.
    let mut c = Client::signed_in().await;
    c.send("POST", "/api/v1/auth/logout", None).await;
    c.send(
        "POST",
        "/api/v1/auth/login",
        Some(Client::credentials("admin", "wrong password here")),
    )
    .await;
    c.send(
        "POST",
        "/api/v1/auth/login",
        Some(Client::credentials("admin", PASSWORD)),
    )
    .await;

    let (_, body) = c.send("GET", "/api/v1/audit", None).await;
    let actions: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert!(actions.contains(&"failed sign in"), "got {actions:?}");
}

#[tokio::test]
async fn a_secret_never_reaches_the_audit_trail() {
    let mut c = Client::signed_in().await;
    c.send(
        "POST",
        "/api/v1/credentials",
        Some(json!({ "name": "github", "username": "x", "secret": TOKEN })),
    )
    .await;
    let (_, stack) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(Client::stack("App", COMPOSE)),
        )
        .await;
    let id = stack["id"].as_i64().unwrap();
    c.send(
        "PUT",
        &format!("/api/v1/stacks/{id}/env/API_KEY"),
        Some(json!({ "value": "sk_live_secret" })),
    )
    .await;

    let (_, body) = c.send("GET", "/api/v1/audit", None).await;
    let rendered = body.to_string();

    assert!(!rendered.contains(TOKEN), "a credential reached the trail");
    assert!(
        !rendered.contains("sk_live_secret"),
        "a variable's value reached the trail"
    );
    assert!(rendered.contains("API_KEY"), "the name is recorded");
    assert!(rendered.contains("add credential"));
}

#[tokio::test]
async fn the_audit_trail_requires_authentication() {
    let mut c = Client::new().await;
    c.send(
        "POST",
        "/api/v1/auth/bootstrap",
        Some(Client::credentials("admin", PASSWORD)),
    )
    .await;
    c.send("POST", "/api/v1/auth/logout", None).await;

    let (status, _) = c.send("GET", "/api/v1/audit", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_shell_handshake_from_another_origin_is_refused() {
    // WebSocket handshakes bypass the same-origin policy, so without this a
    // malicious page could open a root shell using the visitor's cookie.
    let c = Client::signed_in().await;

    let request = Request::builder()
        .method("GET")
        .uri("/api/v1/hosts/1/containers/abc/exec")
        .header("cookie", c.cookie.clone().expect("a session"))
        .header("host", "ghostdock.example")
        .header("origin", "http://evil.example")
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(Body::empty())
        .expect("request");

    let response = c.router.clone().oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_shell_handshake_from_ghostdocks_own_pages_is_allowed_through_the_origin_check() {
    // It still fails later for want of a daemon, but it must get past the
    // origin gate rather than being refused as cross-site.
    let c = Client::signed_in().await;

    let request = Request::builder()
        .method("GET")
        .uri("/api/v1/hosts/1/containers/abc/exec")
        .header("cookie", c.cookie.clone().expect("a session"))
        .header("host", "ghostdock.example")
        .header("origin", "http://ghostdock.example")
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(Body::empty())
        .expect("request");

    let response = c.router.clone().oneshot(request).await.expect("response");
    assert_ne!(
        response.status(),
        StatusCode::FORBIDDEN,
        "a same-origin handshake must not be refused as cross-site"
    );
    // 426 is the harness, not the app: `oneshot` has no real connection for
    // the upgrade extractor to take over, so it declines after the origin
    // check has already passed. What this test proves is that the origin
    // gate let it through.
    assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
}

#[tokio::test]
async fn an_unauthenticated_shell_handshake_is_refused() {
    let mut c = Client::new().await;
    c.send(
        "POST",
        "/api/v1/auth/bootstrap",
        Some(Client::credentials("admin", PASSWORD)),
    )
    .await;
    c.send("POST", "/api/v1/auth/logout", None).await;

    let request = Request::builder()
        .method("GET")
        .uri("/api/v1/hosts/1/containers/abc/exec")
        .header("host", "ghostdock.example")
        .header("origin", "http://ghostdock.example")
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(Body::empty())
        .expect("request");

    let response = c.router.clone().oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// ---- accounts -----------------------------------------------------------

const OTHER_PASSWORD: &str = "another long passphrase";

#[tokio::test]
async fn a_second_account_can_be_added_and_can_sign_in() {
    let mut c = Client::signed_in().await;
    let (status, body) = c
        .send(
            "POST",
            "/api/v1/users",
            Some(Client::credentials("second", OTHER_PASSWORD)),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["username"], json!("second"));
    assert!(body.get("password").is_none() && body.get("password_hash").is_none());

    let (_, list) = c.send("GET", "/api/v1/users", None).await;
    let names: Vec<_> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|u| {
            (
                u["username"].as_str().unwrap().to_owned(),
                u["you"] == json!(true),
            )
        })
        .collect();
    assert_eq!(
        names,
        [("admin".to_owned(), true), ("second".to_owned(), false)]
    );

    let mut other = c.sibling();
    let (status, _) = other
        .send(
            "POST",
            "/api/v1/auth/login",
            Some(Client::credentials("second", OTHER_PASSWORD)),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_new_account_meets_the_same_password_policy_and_needs_a_unique_name() {
    let mut c = Client::signed_in().await;
    let (status, _) = c
        .send(
            "POST",
            "/api/v1/users",
            Some(Client::credentials("short", "tiny")),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = c
        .send(
            "POST",
            "/api/v1/users",
            Some(Client::credentials("ADMIN", OTHER_PASSWORD)),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, _) = c
        .send(
            "POST",
            "/api/v1/users",
            Some(Client::credentials("  ", OTHER_PASSWORD)),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn removing_an_account_ends_its_sessions_at_once() {
    let mut c = Client::signed_in().await;
    let (_, second) = c
        .send(
            "POST",
            "/api/v1/users",
            Some(Client::credentials("second", OTHER_PASSWORD)),
        )
        .await;
    let mut other = c.sibling();
    other
        .send(
            "POST",
            "/api/v1/auth/login",
            Some(Client::credentials("second", OTHER_PASSWORD)),
        )
        .await;

    let id = second["id"].as_i64().unwrap();
    let (status, _) = c.send("DELETE", &format!("/api/v1/users/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = other.send("GET", "/api/v1/hosts", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn nobody_can_remove_their_own_account() {
    // Which also means the last account can never be removed through the
    // API: whoever is asking is always one of the accounts that remain.
    let mut c = Client::signed_in().await;
    let (_, me) = c.send("GET", "/api/v1/auth/status", None).await;
    let id = me["user"]["id"].as_i64().unwrap();

    let (status, _) = c.send("DELETE", &format!("/api/v1/users/{id}"), None).await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, body) = c.send("GET", "/api/v1/auth/status", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["bootstrapped"], json!(true), "setup must not reopen");
}

#[tokio::test]
async fn removing_an_unknown_account_is_not_found() {
    let mut c = Client::signed_in().await;
    let (status, _) = c.send("DELETE", "/api/v1/users/999", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn changing_a_password_needs_the_current_one() {
    let mut c = Client::signed_in().await;
    let (status, _) = c
        .send(
            "PUT",
            "/api/v1/auth/password",
            Some(json!({ "current": "not the password", "new": OTHER_PASSWORD })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = c
        .send(
            "PUT",
            "/api/v1/auth/password",
            Some(json!({ "current": PASSWORD, "new": "tiny" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn changing_a_password_signs_out_every_other_session_but_this_one() {
    let mut c = Client::signed_in().await;
    let mut phone = c.sibling();
    phone
        .send(
            "POST",
            "/api/v1/auth/login",
            Some(Client::credentials("admin", PASSWORD)),
        )
        .await;
    assert_eq!(
        phone.send("GET", "/api/v1/hosts", None).await.0,
        StatusCode::OK
    );

    let (status, _) = c
        .send(
            "PUT",
            "/api/v1/auth/password",
            Some(json!({ "current": PASSWORD, "new": OTHER_PASSWORD })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert_eq!(c.send("GET", "/api/v1/hosts", None).await.0, StatusCode::OK);
    assert_eq!(
        phone.send("GET", "/api/v1/hosts", None).await.0,
        StatusCode::UNAUTHORIZED
    );

    let mut fresh = c.sibling();
    let (old, _) = fresh
        .send(
            "POST",
            "/api/v1/auth/login",
            Some(Client::credentials("admin", PASSWORD)),
        )
        .await;
    assert_eq!(old, StatusCode::UNAUTHORIZED);
    let (new, _) = fresh
        .send(
            "POST",
            "/api/v1/auth/login",
            Some(Client::credentials("admin", OTHER_PASSWORD)),
        )
        .await;
    assert_eq!(new, StatusCode::OK);
}

#[tokio::test]
async fn account_management_requires_authentication() {
    let mut c = Client::signed_in().await;
    c.send("POST", "/api/v1/auth/logout", None).await;
    for (method, uri) in [
        ("GET", "/api/v1/users"),
        ("POST", "/api/v1/users"),
        ("DELETE", "/api/v1/users/1"),
        ("PUT", "/api/v1/auth/password"),
    ] {
        let (status, _) = c.send(method, uri, Some(json!({}))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

// ---- API tokens ---------------------------------------------------------

impl Client {
    /// Issues a token from this (signed-in) client and returns a sibling
    /// client that presents it and nothing else.
    async fn with_token(&mut self, name: &str, permissions: &[&str]) -> Self {
        let (status, body) = self
            .send(
                "POST",
                "/api/v1/tokens",
                Some(json!({ "name": name, "permissions": permissions, "expires_in_days": null })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let mut other = self.sibling();
        other.bearer = Some(body["secret"].as_str().unwrap().to_owned());
        other
    }
}

#[tokio::test]
async fn a_token_secret_is_shown_once_and_never_listed() {
    let mut c = Client::signed_in().await;
    let (status, body) = c
        .send(
            "POST",
            "/api/v1/tokens",
            Some(
                json!({ "name": "assistant", "permissions": ["host.view"], "expires_in_days": 30 }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let secret = body["secret"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("ghostdock_"));
    assert!(body["token"]["expires_at"].is_string());

    let (_, list) = c.send("GET", "/api/v1/tokens", None).await;
    let text = list.to_string();
    assert!(!text.contains(&secret), "the secret must not be listed");
    assert_eq!(list[0]["name"], json!("assistant"));
    assert_eq!(list[0]["permissions"], json!(["host.view"]));
}

#[tokio::test]
async fn a_token_can_do_what_it_was_granted_and_nothing_more() {
    let mut c = Client::signed_in().await;
    let mut reader = c.with_token("reader", &["host.view"]).await;

    assert_eq!(
        reader.send("GET", "/api/v1/hosts", None).await.0,
        StatusCode::OK
    );

    let (status, body) = reader
        .send(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(Client::stack("Blog", COMPOSE)),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], json!("missing_permission"));
    assert!(body["message"].as_str().unwrap().contains("stacks.create"));

    let mut editor = c.with_token("creator", &["stacks.create"]).await;
    let (status, _) = editor
        .send(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(Client::stack("Blog", COMPOSE)),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    // Creating does not imply viewing.
    assert_eq!(
        editor.send("GET", "/api/v1/stacks/1", None).await.0,
        StatusCode::FORBIDDEN
    );
}

/// One route guarded by each permission. Every permission appears exactly
/// once, so adding a permission without a guarded route fails the test below.
const GUARDED: &[(&str, &str, &str)] = &[
    ("host.view", "GET", "/api/v1/hosts"),
    ("stacks.read_compose", "GET", "/api/v1/stacks/1/compose"),
    ("logs.view", "GET", "/api/v1/hosts/1/containers/x/logs"),
    ("activity.view", "GET", "/api/v1/audit"),
    ("stacks.deploy", "POST", "/api/v1/stacks/1/deploy"),
    ("stacks.restart", "POST", "/api/v1/stacks/1/restart"),
    ("stacks.stop", "POST", "/api/v1/stacks/1/stop"),
    ("stacks.take_down", "POST", "/api/v1/stacks/1/down"),
    ("updates.check", "POST", "/api/v1/stacks/1/check"),
    ("updates.auto_apply", "PUT", "/api/v1/stacks/1/auto-apply"),
    ("stacks.create", "POST", "/api/v1/hosts/1/stacks"),
    ("stacks.edit", "PUT", "/api/v1/stacks/1"),
    ("stacks.forget", "DELETE", "/api/v1/stacks/1"),
    ("env.write", "PUT", "/api/v1/stacks/1/env/KEY"),
    ("repos.manage", "POST", "/api/v1/repos"),
    ("credentials.manage", "POST", "/api/v1/credentials"),
    ("cleanup.run", "POST", "/api/v1/hosts/1/cleanup"),
    ("shell.open", "GET", "/api/v1/hosts/1/containers/x/exec"),
];

fn all_permissions() -> Vec<&'static str> {
    GUARDED.iter().map(|(p, _, _)| *p).collect()
}

#[tokio::test]
async fn every_permission_has_a_guarded_route() {
    let mut listed = all_permissions();
    listed.sort_unstable();
    let mut known: Vec<_> = shared::token::Permission::ALL
        .iter()
        .map(|p| p.as_str())
        .collect();
    known.sort_unstable();
    assert_eq!(listed, known);
}

#[tokio::test]
async fn each_permission_is_needed_for_the_routes_it_names() {
    // A token with every permission but one is refused exactly where that
    // one is needed. Refusal comes before the route does any work, and
    // stack 1 does not exist, so none of these touch Docker.
    let mut c = Client::signed_in().await;
    for (missing, method, uri) in GUARDED {
        let granted: Vec<&str> = all_permissions()
            .into_iter()
            .filter(|p| p != missing)
            .collect();
        let mut t = c.with_token(&format!("without {missing}"), &granted).await;
        let (status, body) = t.send(method, uri, Some(json!({}))).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {uri} without {missing}: {body}"
        );
        assert_eq!(body["code"], json!("missing_permission"), "{method} {uri}");
    }
}

#[tokio::test]
async fn each_permission_is_enough_on_its_own_for_the_routes_it_names() {
    let mut c = Client::signed_in().await;
    for (only, method, uri) in GUARDED {
        let mut t = c.with_token(&format!("only {only}"), &[only]).await;
        let (status, body) = t.send(method, uri, Some(json!({}))).await;
        assert!(
            body["code"] != json!("missing_permission") && body["code"] != json!("session_only"),
            "{method} {uri} with only {only}: {status} {body}"
        );
    }
}

#[tokio::test]
async fn no_token_can_manage_accounts_or_tokens() {
    let mut c = Client::signed_in().await;
    let mut t = c.with_token("everything", &all_permissions()).await;
    for (method, uri, body) in [
        ("GET", "/api/v1/tokens", None),
        (
            "POST",
            "/api/v1/tokens",
            Some(json!({ "name": "more", "permissions": ["host.view"], "expires_in_days": null })),
        ),
        ("DELETE", "/api/v1/tokens/1", None),
        ("GET", "/api/v1/users", None),
        (
            "POST",
            "/api/v1/users",
            Some(Client::credentials("sneaky", OTHER_PASSWORD)),
        ),
        (
            "PUT",
            "/api/v1/auth/password",
            Some(json!({ "current": PASSWORD, "new": OTHER_PASSWORD })),
        ),
    ] {
        let (status, reply) = t.send(method, uri, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
        assert_eq!(reply["code"], json!("session_only"), "{method} {uri}");
    }
}

#[tokio::test]
async fn a_bad_token_is_refused_even_alongside_a_good_cookie() {
    // A client that presents a token means to act as that token. Falling
    // back to a cookie would let a revoked token keep working in a browser.
    let mut c = Client::signed_in().await;
    c.bearer = Some("ghostdock_not-a-real-token".to_owned());
    assert_eq!(
        c.send("GET", "/api/v1/hosts", None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_revoked_token_stops_working_at_once() {
    let mut c = Client::signed_in().await;
    let mut t = c.with_token("short-lived", &["host.view"]).await;
    assert_eq!(t.send("GET", "/api/v1/hosts", None).await.0, StatusCode::OK);

    let (_, list) = c.send("GET", "/api/v1/tokens", None).await;
    let id = list[0]["id"].as_i64().unwrap();
    let (status, _) = c
        .send("DELETE", &format!("/api/v1/tokens/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert_eq!(
        t.send("GET", "/api/v1/hosts", None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_token_needs_a_name_and_a_permission_and_a_sane_expiry() {
    let mut c = Client::signed_in().await;
    for body in [
        json!({ "name": " ", "permissions": ["host.view"], "expires_in_days": null }),
        json!({ "name": "x", "permissions": [], "expires_in_days": null }),
        json!({ "name": "x", "permissions": ["host.view"], "expires_in_days": 0 }),
        json!({ "name": "x", "permissions": ["host.view"], "expires_in_days": 100000 }),
    ] {
        let (status, _) = c.send("POST", "/api/v1/tokens", Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
    let (status, _) = c
        .send(
            "POST",
            "/api/v1/tokens",
            Some(
                json!({ "name": "x", "permissions": ["accounts.manage"], "expires_in_days": null }),
            ),
        )
        .await;
    assert!(
        status.is_client_error(),
        "an unknown permission is refused: {status}"
    );
}

#[tokio::test]
async fn what_a_token_does_is_recorded_under_its_name() {
    let mut c = Client::signed_in().await;
    let mut t = c.with_token("assistant", &["stacks.create"]).await;
    t.send(
        "POST",
        "/api/v1/hosts/1/stacks",
        Some(Client::stack("Blog", COMPOSE)),
    )
    .await;

    let (_, audit) = c.send("GET", "/api/v1/audit", None).await;
    let who: Vec<_> = audit
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["action"] != json!("create token"))
        .map(|e| e["username"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        who.contains(&"admin (token assistant)".to_owned()),
        "{who:?}"
    );
}

#[tokio::test]
async fn revoking_a_token_closes_its_open_event_stream() {
    let mut c = Client::signed_in().await;
    let t = c.with_token("watcher", &["host.view"]).await;

    let req = Request::builder()
        .uri("/api/v1/events")
        .header(
            "authorization",
            format!("Bearer {}", t.bearer.as_deref().unwrap()),
        )
        .body(Body::empty())
        .unwrap();
    let res = c.router.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let mut body = res.into_body();

    let (_, list) = c.send("GET", "/api/v1/tokens", None).await;
    let id = list[0]["id"].as_i64().unwrap();
    c.send("DELETE", &format!("/api/v1/tokens/{id}"), None)
        .await;

    // The stream must end rather than idle on with keep-alives.
    let ended = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(frame) = body.frame().await {
            if frame.is_err() {
                break;
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the stream outlived its token");
}

#[tokio::test]
async fn following_logs_needs_the_logs_permission_and_a_daemon() {
    let mut c = Client::signed_in().await;
    let uri = "/api/v1/hosts/1/containers/x/logs/follow";
    let mut t = c.with_token("no logs", &["host.view"]).await;
    let (status, body) = t.send("GET", uri, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], json!("missing_permission"));

    // No daemon in these tests: a clear 503, not a stream that never speaks.
    let (status, _) = c.send("GET", uri, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn container_cleanup_scopes_are_understood() {
    // No daemon here, so each reaches the daemon check rather than being
    // rejected as an unknown scope.
    let mut c = Client::signed_in().await;
    for scope in ["leftover", "standalone"] {
        let (status, _) = c
            .send(
                "POST",
                "/api/v1/hosts/1/cleanup",
                Some(json!({ "scope": scope })),
            )
            .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{scope}");
    }
}

// ---- discovering stacks in a repository ---------------------------------

/// A real repository with the usual layouts: one file per stack under
/// `compose/`, a stack in its own directory, and a root compose file.
fn discovery_repo() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "--quiet", "--initial-branch", "main", "."]);
    for (path, body) in [
        ("compose/blog.yml", COMPOSE),
        ("compose/wiki.yaml", COMPOSE),
        ("apps/notes/compose.yaml", COMPOSE),
        ("compose.yaml", COMPOSE),
        ("README.md", "not a stack"),
    ] {
        let full = dir.path().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, body).unwrap();
    }
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "stacks"]);
    let url = format!("file://{}/infra.git", dir.path().display());
    // A URL ending in .git, as most do, served from the directory itself.
    std::os::unix::fs::symlink(dir.path(), dir.path().join("infra.git")).unwrap();
    (dir, url)
}

fn found(body: &Value) -> Vec<(String, String, String)> {
    body["found"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["path"].as_str().unwrap().to_owned(),
                f["name"].as_str().unwrap().to_owned(),
                f["status"]["kind"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn discovery_finds_the_usual_layouts_and_names_each_stack() {
    let (_dir, url) = discovery_repo();
    let mut c = Client::signed_in().await;
    let repo = c.repo(&url, None).await;

    let (status, body) = c
        .send(
            "POST",
            &format!("/api/v1/repos/{repo}/discover"),
            Some(json!({ "git_ref": "refs/heads/main", "pattern": null })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["commit"].as_str().unwrap().len(), 40);
    let new = |p: &str, n: &str| (p.to_owned(), n.to_owned(), "new".to_owned());
    assert_eq!(
        found(&body),
        [
            new("apps/notes/compose.yaml", "notes"),
            new("compose.yaml", "infra"),
            new("compose/blog.yml", "blog"),
            new("compose/wiki.yaml", "wiki"),
        ],
        "a root file is named for the repository"
    );
}

#[tokio::test]
async fn importing_registers_what_was_chosen_and_nothing_is_registered_twice() {
    let (_dir, url) = discovery_repo();
    let mut c = Client::signed_in().await;
    let repo = c.repo(&url, None).await;
    // Something already called "wiki", unrelated to this repository.
    c.send(
        "POST",
        "/api/v1/hosts/1/stacks",
        Some(Client::stack("Wiki", COMPOSE)),
    )
    .await;

    let (status, body) = c
        .send(
            "POST",
            &format!("/api/v1/repos/{repo}/import"),
            Some(json!({
                "git_ref": "refs/heads/main",
                "pattern": null,
                "paths": ["compose/blog.yml", "compose/wiki.yaml", "../etc/passwd", "README.md"],
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let created: Vec<_> = body["created"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["slug"].as_str().unwrap(),
                s["git"]["compose_path"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(created, [("blog", "compose/blog.yml")]);
    let skipped: Vec<_> = body["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s[0].as_str().unwrap())
        .collect();
    assert_eq!(skipped, ["compose/wiki.yaml", "../etc/passwd", "README.md"]);

    let (_, again) = c
        .send(
            "POST",
            &format!("/api/v1/repos/{repo}/discover"),
            Some(json!({ "git_ref": "refs/heads/main", "pattern": null })),
        )
        .await;
    let status_of = |path: &str| {
        found(&again)
            .into_iter()
            .find(|(p, _, _)| p == path)
            .map(|(_, _, s)| s)
            .unwrap()
    };
    assert_eq!(status_of("compose/blog.yml"), "registered");
    assert_eq!(status_of("compose/wiki.yaml"), "name_taken");
    assert_eq!(status_of("apps/notes/compose.yaml"), "new");
}

#[tokio::test]
async fn a_custom_pattern_narrows_discovery() {
    let (_dir, url) = discovery_repo();
    let mut c = Client::signed_in().await;
    let repo = c.repo(&url, None).await;
    let (_, body) = c
        .send(
            "POST",
            &format!("/api/v1/repos/{repo}/discover"),
            Some(json!({ "git_ref": "refs/heads/main", "pattern": "compose/*.yml" })),
        )
        .await;
    let paths: Vec<_> = found(&body).into_iter().map(|(p, _, _)| p).collect();
    assert_eq!(paths, ["compose/blog.yml"]);
    assert_eq!(body["pattern"], json!("compose/*.yml"));
}

#[tokio::test]
async fn discovery_reports_bad_input_plainly() {
    let (_dir, url) = discovery_repo();
    let mut c = Client::signed_in().await;
    let repo = c.repo(&url, None).await;
    let discover =
        |pattern: Value, git_ref: &str| json!({ "git_ref": git_ref, "pattern": pattern });

    let (status, _) = c
        .send(
            "POST",
            "/api/v1/repos/999/discover",
            Some(discover(Value::Null, "refs/heads/main")),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = c
        .send(
            "POST",
            &format!("/api/v1/repos/{repo}/discover"),
            Some(discover(json!("{a,b"), "refs/heads/main")),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = c
        .send(
            "POST",
            &format!("/api/v1/repos/{repo}/discover"),
            Some(discover(Value::Null, "refs/heads/nope")),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn discovering_and_importing_need_stacks_create() {
    let mut c = Client::signed_in().await;
    let mut t = c.with_token("viewer", &["host.view"]).await;
    for uri in ["/api/v1/repos/1/discover", "/api/v1/repos/1/import"] {
        let (status, body) = t.send("POST", uri, Some(json!({}))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(body["code"], json!("missing_permission"));
    }
}

#[tokio::test]
async fn a_repositorys_credential_can_be_changed_or_removed() {
    let mut c = Client::signed_in().await;
    let (_, old) = c
        .send(
            "POST",
            "/api/v1/credentials",
            Some(json!({ "name": "old", "username": "u", "secret": "s1" })),
        )
        .await;
    let (_, new) = c
        .send(
            "POST",
            "/api/v1/credentials",
            Some(json!({ "name": "new", "username": "u", "secret": "s2" })),
        )
        .await;
    let repo = c
        .repo("https://example.invalid/r.git", old["id"].as_i64())
        .await;

    let (status, body) = c
        .send(
            "PUT",
            &format!("/api/v1/repos/{repo}"),
            Some(json!({ "credential_id": new["id"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["credential_name"], json!("new"));

    let (status, body) = c
        .send(
            "PUT",
            &format!("/api/v1/repos/{repo}"),
            Some(json!({ "credential_id": null })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["credential_id"], Value::Null);

    // With nothing referring to it any more, the old one can go.
    let (status, _) = c
        .send(
            "DELETE",
            &format!("/api/v1/credentials/{}", old["id"]),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = c
        .send(
            "PUT",
            &format!("/api/v1/repos/{repo}"),
            Some(json!({ "credential_id": 999 })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = c
        .send(
            "PUT",
            "/api/v1/repos/999",
            Some(json!({ "credential_id": null })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_import_finishes_even_if_the_caller_goes_away() {
    // Closing the tab drops the request. Work already asked for must still
    // complete, not stop halfway with some stacks registered and some not.
    let (_dir, url) = discovery_repo();
    let mut c = Client::signed_in().await;
    let repo = c.repo(&url, None).await;

    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/repos/{repo}/import"))
        .header("content-type", "application/json");
    if let Some(cookie) = &c.cookie {
        req = req.header("cookie", cookie);
    }
    let req = req
        .body(Body::from(
            json!({
                "git_ref": "refs/heads/main",
                "pattern": null,
                "paths": ["compose/blog.yml", "compose/wiki.yaml"],
            })
            .to_string(),
        ))
        .unwrap();
    // Dropped long before git could have fetched anything.
    let abandoned = tokio::time::timeout(
        std::time::Duration::from_millis(1),
        c.router.clone().oneshot(req),
    )
    .await;
    assert!(
        abandoned.is_err(),
        "the request should still have been running"
    );

    let mut registered = Vec::new();
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        // The updates list names every registered stack, and needs no daemon.
        let (_, updates) = c.send("GET", "/api/v1/hosts/1/updates", None).await;
        registered = updates
            .as_array()
            .map(|s| {
                s.iter()
                    .filter_map(|s| s["stack"]["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        if registered.len() == 2 {
            break;
        }
    }
    registered.sort();
    assert_eq!(registered, ["blog", "wiki"]);
}

#[tokio::test]
async fn running_a_command_needs_shell_open_and_a_daemon() {
    let mut c = Client::signed_in().await;
    let uri = "/api/v1/hosts/1/containers/x/exec/run";
    let body = json!({ "command": "id", "timeout_seconds": 5 });

    let mut t = c.with_token("no shell", &["host.view", "logs.view"]).await;
    let (status, reply) = t.send("POST", uri, Some(body.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(reply["code"], json!("missing_permission"));

    let (status, _) = c.send("POST", uri, Some(body)).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "no daemon in tests"
    );

    let (status, _) = c
        .send(
            "POST",
            uri,
            Some(json!({ "command": "  ", "timeout_seconds": null })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---- operations on what was deployed --------------------------------------

fn daemon_available() -> bool {
    std::process::Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .output()
        .is_ok_and(|o| o.status.success())
}

fn git_in(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

impl Client {
    /// Runs an operation and waits for its outcome.
    async fn operate(&mut self, stack: i64, verb: &str) -> Value {
        let (status, started) = self
            .send("POST", &format!("/api/v1/stacks/{stack}/{verb}"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{verb}: {started}");
        let id = started["id"].as_i64().unwrap();
        for _ in 0..600 {
            let (_, detail) = self
                .send("GET", &format!("/api/v1/deployments/{id}"), None)
                .await;
            if detail["status"] != json!("running") {
                return detail;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        panic!("{verb} did not finish");
    }
}

#[tokio::test]
async fn restarting_does_not_fetch_or_mark_the_stack_up_to_date() {
    // Only a deploy changes what a stack runs. Restarting after a push must
    // leave the new commit waiting, not claim it was applied.
    if !daemon_available() {
        eprintln!("SKIPPED: no Docker daemon reachable");
        return;
    }
    let origin = tempfile::tempdir().unwrap();
    std::fs::write(
        origin.path().join("compose.yaml"),
        "services:\n  app:\n    image: alpine:3.22\n    command: [\"sleep\", \"3600\"]\n",
    )
    .unwrap();
    git_in(
        origin.path(),
        &["init", "--quiet", "--initial-branch", "main", "."],
    );
    git_in(origin.path(), &["add", "."]);
    git_in(origin.path(), &["commit", "--quiet", "-m", "first"]);
    let url = format!("file://{}", origin.path().display());

    let mut c = Client::signed_in().await;
    let repo = c.repo(&url, None).await;
    let (status, stack) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks/git",
            Some(json!({
                "name": "ghostdocktest-restart-behind", "repo_id": repo,
                "git_ref": "refs/heads/main", "compose_path": "compose.yaml"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{stack}");
    let id = stack["id"].as_i64().unwrap();

    let deployed = c.operate(id, "deploy").await;
    let deployed_ok = deployed["status"] == json!("succeeded");
    let deployed_commit = deployed["commit_sha"].clone();

    std::fs::write(origin.path().join("README.md"), "moved on\n").unwrap();
    git_in(origin.path(), &["add", "."]);
    git_in(origin.path(), &["commit", "--quiet", "-m", "second"]);

    let restarted = c.operate(id, "restart").await;
    let (_, after) = c
        .send("POST", &format!("/api/v1/stacks/{id}/check"), None)
        .await;
    let (_, now) = c.send("GET", &format!("/api/v1/stacks/{id}"), None).await;
    let down = c.operate(id, "down").await;

    assert!(deployed_ok, "{}", deployed["log"]);
    assert_eq!(
        restarted["status"],
        json!("succeeded"),
        "{}",
        restarted["log"]
    );
    assert_eq!(
        restarted["commit_sha"],
        Value::Null,
        "restart fetched nothing"
    );
    assert_eq!(
        now["git"]["last_commit"], deployed_commit,
        "the deployed commit is still the first one"
    );
    assert_eq!(after["deployed_commit"], deployed_commit);
    assert_ne!(
        after["remote_commit"], after["deployed_commit"],
        "the new commit is still waiting: {after}"
    );
    assert_eq!(down["status"], json!("succeeded"), "{}", down["log"]);
}

#[tokio::test]
async fn taking_down_a_stack_never_deployed_from_here_uses_its_name() {
    // A stack imported while already running has no checkout here yet.
    // Taking it down must not fetch one first.
    if !daemon_available() {
        eprintln!("SKIPPED: no Docker daemon reachable");
        return;
    }
    let mut c = Client::signed_in().await;
    // Unreachable on purpose: any fetch would fail the operation.
    let repo = c.repo("file:///nonexistent/ghostdocktest.git", None).await;
    let (_, stack) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks/git",
            Some(json!({
                "name": "ghostdocktest-never-deployed", "repo_id": repo,
                "git_ref": "refs/heads/main", "compose_path": "compose.yaml"
            })),
        )
        .await;
    let id = stack["id"].as_i64().unwrap();

    for verb in ["stop", "restart", "down"] {
        let done = c.operate(id, verb).await;
        assert_eq!(
            done["status"],
            json!("succeeded"),
            "{verb}: {}",
            done["log"]
        );
    }
}

/// A git server that accepts connections and never answers, so anything
/// fetching from it stays busy until the listener is dropped.
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

#[tokio::test]
async fn a_stack_cannot_be_forgotten_while_something_is_running_for_it() {
    let (remote, url) = silent_remote().await;
    let mut c = Client::signed_in().await;
    let repo = c.repo(&url, None).await;
    let (_, stack) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks/git",
            Some(json!({
                "name": "Busy", "repo_id": repo,
                "git_ref": "refs/heads/main", "compose_path": "compose.yaml"
            })),
        )
        .await;
    let id = stack["id"].as_i64().unwrap();

    let (status, _) = c
        .send("POST", &format!("/api/v1/stacks/{id}/deploy"), None)
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = c
        .send("DELETE", &format!("/api/v1/stacks/{id}"), None)
        .await;
    remote.abort();
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, _) = c.send("GET", &format!("/api/v1/stacks/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "still registered");
}

#[tokio::test]
async fn forgetting_a_stack_removes_its_secrets_file_and_nothing_else() {
    let mut c = Client::signed_in().await;
    let (_, stack) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(Client::stack("App", COMPOSE)),
        )
        .await;
    let id = stack["id"].as_i64().unwrap();
    // As a deploy leaves it.
    let dir = c._stacks_root.path().join("app");
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join(".env"), "API_KEY=sk_live_secret\n").unwrap();
    std::fs::write(dir.join("docker-compose.yml"), COMPOSE).unwrap();
    std::fs::write(dir.join("data/app.db"), "keep me").unwrap();

    let (status, _) = c
        .send("DELETE", &format!("/api/v1/stacks/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert!(!dir.join(".env").exists(), "the secrets outlived the stack");
    assert!(dir.join("docker-compose.yml").exists());
    assert!(
        dir.join("data/app.db").exists(),
        "data must never be removed"
    );
}

#[tokio::test]
async fn removing_a_repository_removes_its_discovery_checkout() {
    let (_dir, url) = discovery_repo();
    let mut c = Client::signed_in().await;
    let repo = c.repo(&url, None).await;
    c.send(
        "POST",
        &format!("/api/v1/repos/{repo}/discover"),
        Some(json!({ "git_ref": "refs/heads/main", "pattern": null })),
    )
    .await;
    let checkout = c
        ._stacks_root
        .path()
        .join(".discovery")
        .join(repo.to_string());
    assert!(checkout.join(".git").exists(), "discovery checked it out");

    let (status, _) = c
        .send("DELETE", &format!("/api/v1/repos/{repo}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!checkout.exists());
}

#[tokio::test]
async fn every_change_to_a_stack_or_its_sources_is_recorded_without_values() {
    let mut c = Client::signed_in().await;
    let (_, stack) = c
        .send(
            "POST",
            "/api/v1/hosts/1/stacks",
            Some(Client::stack("App", COMPOSE)),
        )
        .await;
    let id = stack["id"].as_i64().unwrap();
    let repo = c.repo("https://example.invalid/r.git", None).await;

    c.send(
        "PUT",
        &format!("/api/v1/stacks/{id}"),
        Some(Client::stack("App", "services: {}  # sk_in_compose\n")),
    )
    .await;
    c.send(
        "PUT",
        &format!("/api/v1/stacks/{id}/env"),
        Some(json!({ "vars": [{ "key": "API_KEY", "value": "sk_live_secret" }] })),
    )
    .await;
    c.send("DELETE", &format!("/api/v1/stacks/{id}/env/API_KEY"), None)
        .await;
    c.send(
        "PUT",
        &format!("/api/v1/stacks/{id}/auto-apply"),
        Some(json!({ "enabled": true })),
    )
    .await;
    c.send("DELETE", &format!("/api/v1/repos/{repo}"), None)
        .await;

    let (_, body) = c.send("GET", "/api/v1/audit", None).await;
    let entries: Vec<(String, String)> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["action"].as_str().unwrap().to_owned(),
                e["target"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    for (action, target) in [
        ("edit compose file", "app"),
        ("replace variables", "app"),
        ("remove variable", "API_KEY"),
        ("turn on auto-apply", "app"),
        ("remove repository", "https://example.invalid/r.git"),
    ] {
        assert!(
            entries.contains(&(action.to_owned(), target.to_owned())),
            "no {action} of {target} in {entries:?}"
        );
    }
    let rendered = body.to_string();
    assert!(rendered.contains("API_KEY"), "names are recorded");
    assert!(!rendered.contains("sk_live_secret"), "a value was recorded");
    assert!(!rendered.contains("sk_in_compose"), "the file was recorded");
}
