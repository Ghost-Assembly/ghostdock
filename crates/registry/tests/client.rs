//! Against a registry that speaks the real protocol.
//!
//! A local server rather than Docker Hub: hermetic, no network, and it does
//! not spend the anonymous rate limit that this code exists to be careful
//! with. The token dance is genuinely exercised, which is the part worth
//! testing.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use registry::reference::parse;
use registry::{Client, Error};

const DIGEST: &str = "sha256:0123456789abcdef";

#[derive(Clone, Default)]
struct Fake {
    /// Counts manifest requests, so pacing can be observed.
    manifest_hits: Arc<AtomicUsize>,
    token_hits: Arc<AtomicUsize>,
    /// The fake's own address, so its challenge can point at itself and the
    /// token exchange is genuinely followed rather than merely attempted.
    base: Arc<Mutex<String>>,
}

async fn token(State(fake): State<Fake>) -> impl IntoResponse {
    fake.token_hits.fetch_add(1, Ordering::Relaxed);
    axum::Json(serde_json::json!({ "token": "a-test-token" }))
}

async fn manifest(
    State(fake): State<Fake>,
    Path((name, extra, target)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    fake.manifest_hits.fetch_add(1, Ordering::Relaxed);
    let repository = format!("{name}/{extra}");

    if target == "gone" {
        return (StatusCode::NOT_FOUND, HeaderMap::new()).into_response();
    }
    if target == "throttled" {
        return (StatusCode::TOO_MANY_REQUESTS, HeaderMap::new()).into_response();
    }
    if repository.ends_with("/private") && !headers.contains_key(header::AUTHORIZATION) {
        // Exactly what a real registry does: say where to get a token.
        let base = fake.base.lock().expect("base").clone();
        let mut out = HeaderMap::new();
        out.insert(
            header::WWW_AUTHENTICATE,
            format!(
                "Bearer realm=\"{base}/token\",service=\"test\",scope=\"repository:{repository}:pull\""
            )
            .parse()
            .unwrap(),
        );
        return (StatusCode::UNAUTHORIZED, out).into_response();
    }

    let mut out = HeaderMap::new();
    out.insert("docker-content-digest", DIGEST.parse().unwrap());
    (StatusCode::OK, out).into_response()
}

/// Starts the fake and returns its base URL.
///
/// The listener is bound first so the fake can be told its own address,
/// which is what lets its challenge point back at itself.
async fn serve(fake: Fake) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    *fake.base.lock().expect("base") = base.clone();

    let app = Router::new()
        .route("/token", get(token))
        .route(
            "/v2/{name}/{extra}/manifests/{target}",
            get(manifest).head(manifest),
        )
        .with_state(fake);

    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    base
}

fn client(base: &str) -> Client {
    Client::new()
        .with_base_override(base.to_owned())
        .with_min_interval(Duration::from_millis(1))
}

#[tokio::test]
async fn reads_the_digest_for_a_public_image() {
    let base = serve(Fake::default()).await;
    let reference = parse("example.test/team/app:1.0").unwrap();

    let digest = client(&base).digest(&reference).await.unwrap();

    assert_eq!(digest, DIGEST);
}

#[tokio::test]
async fn follows_the_token_challenge_for_a_private_image() {
    // The whole dance: unauthorised, read where to ask, fetch a token, retry
    // with it. A registry does not publish a fixed token address, so this
    // cannot be short-circuited by guessing one.
    let fake = Fake::default();
    let base = serve(fake.clone()).await;
    let reference = parse("example.test/team/private:1.0").unwrap();

    let digest = client(&base).digest(&reference).await.unwrap();

    assert_eq!(digest, DIGEST);
    assert_eq!(
        fake.token_hits.load(Ordering::Relaxed),
        1,
        "a token should have been fetched exactly once"
    );
    assert_eq!(
        fake.manifest_hits.load(Ordering::Relaxed),
        2,
        "once unauthorised, once with the token"
    );
}

#[tokio::test]
async fn a_missing_tag_says_which_one() {
    let base = serve(Fake::default()).await;
    let reference = parse("example.test/team/app:gone").unwrap();

    match client(&base).digest(&reference).await {
        Err(Error::NotFound {
            target, repository, ..
        }) => {
            assert_eq!(target, "gone");
            assert_eq!(repository, "team/app");
        }
        other => panic!("expected a not-found naming the tag, got {other:?}"),
    }
}

#[tokio::test]
async fn rate_limiting_is_reported_as_itself() {
    // Distinguished from other failures because it is temporary and the
    // right response is to wait, not to alarm anyone.
    let base = serve(Fake::default()).await;
    let reference = parse("example.test/team/app:throttled").unwrap();

    assert!(matches!(
        client(&base).digest(&reference).await,
        Err(Error::RateLimited(_))
    ));
}

#[tokio::test]
async fn requests_to_one_registry_are_spaced_out() {
    let fake = Fake::default();
    let base = serve(fake.clone()).await;
    let reference = parse("example.test/team/app:1.0").unwrap();
    let client = Client::new()
        .with_base_override(base)
        .with_min_interval(Duration::from_millis(80));

    let started = std::time::Instant::now();
    for _ in 0..3 {
        client.digest(&reference).await.unwrap();
    }

    assert!(
        started.elapsed() >= Duration::from_millis(160),
        "three requests must be spaced by the minimum interval, took {:?}",
        started.elapsed()
    );
    assert_eq!(fake.manifest_hits.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn an_unreachable_registry_names_itself() {
    let reference = parse("example.test/team/app:1.0").unwrap();
    let client = Client::new()
        .with_base_override("http://127.0.0.1:1".to_owned())
        .with_min_interval(Duration::from_millis(1));

    match client.digest(&reference).await {
        Err(Error::Unreachable { registry, .. }) => assert_eq!(registry, "example.test"),
        other => panic!("expected an unreachable error, got {other:?}"),
    }
}
