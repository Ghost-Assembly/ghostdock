//! Probes against real sockets: a local HTTP server and TCP listener
//! started by each test.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use probe::{Client, not_after, tcp};

const SECOND: Duration = Duration::from_secs(1);

/// Serves `router` on a free local port and returns its address.
async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}

/// A port nothing listens on: bound, then let go.
async fn closed_port() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}

fn site() -> Router {
    Router::new()
        .route("/ok", get(|| async { "all systems nominal" }))
        .route("/gone", get(|| async { (StatusCode::NOT_FOUND, "no") }))
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                "late"
            }),
        )
        .route(
            "/big",
            get(|| async { format!("{}needle", "x".repeat(2 << 20)) }),
        )
}

#[tokio::test]
async fn an_http_probe_reports_the_status_and_how_long_it_took() {
    let base = serve(site()).await;
    let client = Client::new();
    let answer = client
        .http(&format!("{base}/ok"), SECOND, None)
        .await
        .unwrap();
    assert_eq!(answer.status, 200);
    assert!(answer.latency < SECOND);
    assert_eq!(answer.keyword_found, None, "none was asked for");
    assert_eq!(answer.tls_days_left, None, "plain HTTP has no certificate");

    let gone = client
        .http(&format!("{base}/gone"), SECOND, None)
        .await
        .unwrap();
    assert_eq!(gone.status, 404, "a status is an answer, judged elsewhere");
}

#[tokio::test]
async fn a_keyword_is_looked_for_in_the_first_mebibyte() {
    let base = serve(site()).await;
    let client = Client::new();
    let found = client
        .http(&format!("{base}/ok"), SECOND, Some("nominal"))
        .await
        .unwrap();
    assert_eq!(found.keyword_found, Some(true));
    let missing = client
        .http(&format!("{base}/ok"), SECOND, Some("on fire"))
        .await
        .unwrap();
    assert_eq!(missing.keyword_found, Some(false));
    // Past the first MiB, so not read.
    let far = client
        .http(
            &format!("{base}/big"),
            Duration::from_secs(5),
            Some("needle"),
        )
        .await
        .unwrap();
    assert_eq!(far.keyword_found, Some(false));
}

#[tokio::test]
async fn a_probe_that_takes_too_long_fails_in_time() {
    let base = serve(site()).await;
    let started = std::time::Instant::now();
    let error = Client::new()
        .http(&format!("{base}/slow"), Duration::from_millis(300), None)
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(error.contains("timed out"), "{error}");
}

#[tokio::test]
async fn a_failure_never_repeats_the_url() {
    // Webhook URLs carry their secret in the path; an error message is
    // shown and stored, so it must not carry it on.
    let url = format!("http://{}/hooks/s3cr3t-token", closed_port().await);
    let error = Client::new().http(&url, SECOND, None).await.unwrap_err();
    assert!(!error.contains("s3cr3t"), "{error}");
    assert!(error.contains("connect"), "{error}");
    let error = Client::new()
        .post(&url, &[], b"{}".to_vec(), SECOND)
        .await
        .unwrap_err();
    assert!(!error.contains("s3cr3t"), "{error}");
}

#[tokio::test]
async fn a_tcp_probe_connects_or_says_why_not() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let open = listener.local_addr().unwrap().to_string();
    let took = tcp(&open, SECOND).await.unwrap();
    assert!(took < SECOND);

    let error = tcp(&closed_port().await, SECOND).await.unwrap_err();
    assert!(error.to_lowercase().contains("refused"), "{error}");
}

#[tokio::test]
async fn a_post_delivers_the_body_and_headers() {
    let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let log = Arc::clone(&seen);
    let router = Router::new()
        .route(
            "/hook",
            post(move |headers: HeaderMap, body: String| {
                let log = Arc::clone(&log);
                async move {
                    let title = headers
                        .get("title")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned();
                    log.lock().unwrap().push((title, body));
                    StatusCode::NO_CONTENT
                }
            }),
        )
        .route(
            "/refuse",
            post(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
        );
    let base = serve(router).await;
    let client = Client::new();

    let status = client
        .post(
            &format!("{base}/hook"),
            &[("Title", "Blog down")],
            b"it broke".to_vec(),
            SECOND,
        )
        .await
        .unwrap();
    assert_eq!(status, 204);
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        [("Blog down".to_owned(), "it broke".to_owned())]
    );

    let error = client
        .post(&format!("{base}/refuse"), &[], Vec::new(), SECOND)
        .await
        .unwrap_err();
    assert!(error.contains("500"), "{error}");
}

// ---- certificates -------------------------------------------------------
//
// A TLS server here would need a certificate and key made in the test,
// which takes a dependency the product does not otherwise need. The part
// that is GhostDock's own, reading when a certificate expires, is tested on
// certificates built here byte by byte.

fn der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = content.len();
    if len < 0x80 {
        out.push(u8::try_from(len).unwrap());
    } else if len < 0x100 {
        out.extend([0x81, u8::try_from(len).unwrap()]);
    } else {
        out.extend([
            0x82,
            u8::try_from(len >> 8).unwrap(),
            u8::try_from(len & 0xff).unwrap(),
        ]);
    }
    out.extend_from_slice(content);
    out
}

/// A certificate's outline, with the fields before the validity filled
/// with plausible values and the validity as given.
fn certificate(version: bool, not_before: &[u8], not_after: &[u8]) -> Vec<u8> {
    let mut tbs = Vec::new();
    if version {
        tbs.extend(der(0xa0, &der(0x02, &[2])));
    }
    tbs.extend(der(0x02, &[0x01, 0x23, 0x45]));
    tbs.extend(der(
        0x30,
        &der(0x06, &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02]),
    ));
    tbs.extend(der(
        0x30,
        &der(
            0x31,
            &der(0x30, &[0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x01, b'x']),
        ),
    ));
    let mut validity = not_before.to_vec();
    validity.extend_from_slice(not_after);
    tbs.extend(der(0x30, &validity));
    // Subject and key: long enough to need a two-byte length.
    tbs.extend(der(0x30, &[0u8; 300]));
    let mut cert = der(0x30, &tbs);
    cert.extend(der(0x30, &der(0x06, &[0x2a])));
    cert.extend(der(0x03, &[0, 1, 2, 3]));
    der(0x30, &cert)
}

#[test]
fn the_expiry_is_read_from_either_kind_of_time() {
    let utc = |t: &str| der(0x17, t.as_bytes());
    let generalized = |t: &str| der(0x18, t.as_bytes());
    let expected = chrono::DateTime::parse_from_rfc3339("2027-01-02T03:04:05Z")
        .unwrap()
        .timestamp();

    let cert = certificate(true, &utc("250101000000Z"), &utc("270102030405Z"));
    assert_eq!(not_after(&cert), Some(expected));
    let cert = certificate(
        false,
        &utc("250101000000Z"),
        &generalized("20270102030405Z"),
    );
    assert_eq!(
        not_after(&cert),
        Some(expected),
        "a v1 certificate has no version"
    );
    // Two-digit years from 50 are the 1900s.
    let old = certificate(true, &utc("500101000000Z"), &utc("991231235959Z"));
    assert_eq!(
        not_after(&old),
        Some(
            chrono::DateTime::parse_from_rfc3339("1999-12-31T23:59:59Z")
                .unwrap()
                .timestamp()
        )
    );
}

#[test]
fn a_damaged_certificate_reads_as_no_expiry() {
    let cert = certificate(
        true,
        &der(0x17, b"250101000000Z"),
        &der(0x17, b"270102030405Z"),
    );
    for cut in [0, 1, 5, 40, cert.len() / 2] {
        assert_eq!(not_after(&cert[..cut]), None, "cut at {cut}");
    }
    let nonsense = certificate(
        true,
        &der(0x17, b"250101000000Z"),
        &der(0x17, b"27x1xx030405Z"),
    );
    assert_eq!(not_after(&nonsense), None);
    assert_eq!(not_after(&[0x30, 0x84, 0xff, 0xff, 0xff, 0xff]), None);
}
