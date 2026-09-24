use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use server::app;
use server::state::AppState;
use shared::metrics::{Reading, SubjectKind};
use store::Store;
use store::metrics::MetricsStore;
use tower::ServiceExt;

async fn setup() -> (axum::Router, String, MetricsStore, tempfile::TempDir) {
    let store = Store::open_in_memory().await.unwrap();
    let metrics = MetricsStore::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let state = AppState::new(store, None, root.path()).with_metrics(metrics.clone());
    let router = app::build(state, false, None);
    let res = router
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/bootstrap")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"username":"admin","password":"correct horse battery staple"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    (router, cookie, metrics, root)
}

async fn get(router: &axum::Router, cookie: &str, uri: &str) -> (StatusCode, Value) {
    let res = router
        .clone()
        .oneshot(
            Request::get(uri)
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_series_never_exceeds_300_points() {
    let (router, cookie, metrics, _root) = setup().await;
    let id = metrics
        .subject(SubjectKind::Container, "busy-1", Some("busy"), None, 0)
        .await
        .unwrap();
    let now = shared_now();
    let rows: Vec<_> = (0..10_080)
        .map(|i| {
            (
                id,
                Reading {
                    t: now - 7 * 86_400 + i * 60,
                    cpu: Some(0.5),
                    cpu_max: Some(0.5),
                    ..Reading::default()
                },
            )
        })
        .collect();
    metrics.write_minute(&rows).await.unwrap();

    let (status, body) = get(
        &router,
        &cookie,
        "/api/v1/hosts/1/metrics/series?subject=container:busy-1&range=7d",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let points = body["points"].as_array().unwrap();
    assert!(
        !points.is_empty() && points.len() <= 300,
        "{}",
        points.len()
    );
    assert_eq!(body["step"], json!(60));

    let (_, stack) = get(
        &router,
        &cookie,
        "/api/v1/hosts/1/metrics/series?subject=stack:busy&range=7d",
    )
    .await;
    assert!(!stack["points"].as_array().unwrap().is_empty());
}

fn shared_now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

#[tokio::test]
async fn a_bad_subject_or_range_is_refused_plainly() {
    let (router, cookie, _m, _root) = setup().await;
    for uri in [
        "/api/v1/hosts/1/metrics/series?subject=planet:earth&range=7d",
        "/api/v1/hosts/1/metrics/series?subject=host&range=forever",
        "/api/v1/hosts/1/metrics/series?range=7d",
    ] {
        let (status, _) = get(&router, &cookie, uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
    }
    let (status, body) = get(
        &router,
        &cookie,
        "/api/v1/hosts/1/metrics/series?subject=container:never-seen&range=24h",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["points"],
        json!([]),
        "nothing measured is an empty series, not an error"
    );
}

#[tokio::test]
async fn now_answers_even_before_the_first_tick() {
    let (router, cookie, _m, _root) = setup().await;
    let (status, body) = get(&router, &cookie, "/api/v1/hosts/1/metrics/now").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["containers"], json!([]));
}

#[tokio::test]
async fn metrics_need_host_view() {
    let (router, _cookie, _m, _root) = setup().await;
    for uri in [
        "/api/v1/hosts/1/metrics/now",
        "/api/v1/hosts/1/sizing",
        "/api/v1/hosts/1/metrics/containers?project=blog&range=24h",
    ] {
        let (status, _) = get(&router, "", uri).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
    }
}

#[tokio::test]
async fn sizing_advises_from_stored_history() {
    let (router, cookie, metrics, _root) = setup().await;
    let now = shared_now();
    let id = metrics
        .subject(
            SubjectKind::Container,
            "blog-web-1",
            Some("blog"),
            Some("web"),
            now,
        )
        .await
        .unwrap();
    let rows: Vec<_> = (0..(4 * 1440))
        .map(|i| {
            (
                id,
                Reading {
                    t: now - 4 * 86_400 + i * 60,
                    cpu: Some(0.2),
                    cpu_max: Some(0.2),
                    mem: Some(300 << 20),
                    mem_max: Some(300 << 20),
                    ..Reading::default()
                },
            )
        })
        .collect();
    metrics.write_minute(&rows).await.unwrap();
    let (status, body) = get(&router, &cookie, "/api/v1/hosts/1/sizing").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body[0]["container"], json!("blog-web-1"));
    assert!(body[0]["snippet"].as_str().unwrap().contains("web:"));
}

#[tokio::test]
async fn a_stacks_containers_come_with_typical_and_peak_figures() {
    let (router, cookie, metrics, _root) = setup().await;
    let now = shared_now();
    let web = metrics
        .subject(
            SubjectKind::Container,
            "blog-web-1",
            Some("blog"),
            Some("web"),
            now,
        )
        .await
        .unwrap();
    let db = metrics
        .subject(
            SubjectKind::Container,
            "blog-db-1",
            Some("blog"),
            Some("db"),
            now,
        )
        .await
        .unwrap();
    let mut rows = Vec::new();
    for i in 0..600 {
        let t = now - 600 * 60 + i * 60;
        let spike = if i == 300 { 2.5 } else { 0.2 };
        rows.push((
            web,
            Reading {
                t,
                cpu: Some(0.2),
                cpu_max: Some(spike),
                mem: Some(300 << 20),
                ..Reading::default()
            },
        ));
        rows.push((
            db,
            Reading {
                t,
                cpu: Some(0.05),
                mem: Some(100 << 20),
                ..Reading::default()
            },
        ));
    }
    metrics.write_minute(&rows).await.unwrap();

    let (status, body) = get(
        &router,
        &cookie,
        "/api/v1/hosts/1/metrics/containers?project=blog&range=24h",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body[0]["key"], json!("blog-db-1"));
    assert_eq!(body[1]["key"], json!("blog-web-1"));
    assert_eq!(body[1]["service"], json!("web"));
    assert_eq!(body[1]["cpu_peak"], json!(2.5));
    assert_eq!(body[1]["mem_typical"], json!(300 << 20));

    for uri in [
        "/api/v1/hosts/1/metrics/containers?range=24h",
        "/api/v1/hosts/1/metrics/containers?project=&range=24h",
        "/api/v1/hosts/1/metrics/containers?project=blog&range=forever",
    ] {
        let (status, _) = get(&router, &cookie, uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
    }
}
