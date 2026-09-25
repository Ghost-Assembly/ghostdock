//! The browser's event socket, over a real listener.
//!
//! A WebSocket cannot be exercised through the router in process, so this
//! serves on a loopback port and connects a real client.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::StreamExt as _;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use server::app;
use server::state::AppState;
use shared::event::{ContainerChange, ServerEvent};
use store::Store;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tower::ServiceExt;

struct Served {
    addr: std::net::SocketAddr,
    router: axum::Router,
    state: AppState,
    _root: tempfile::TempDir,
}

async fn serve() -> Served {
    let store = Store::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let state = AppState::new(store, None, root.path());
    let router = app::build(state.clone(), false, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let served = router.clone();
    tokio::spawn(async move { axum::serve(listener, served).await });
    Served {
        addr,
        router,
        state,
        _root: root,
    }
}

async fn call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Value,
) -> (StatusCode, Value, Option<String>) {
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        set,
    )
}

/// Bootstraps and issues a token with `permissions`; returns the secret.
async fn token(s: &Served, permissions: &[&str]) -> (String, String) {
    let (_, _, cookie) = call(
        &s.router,
        "POST",
        "/api/v1/auth/bootstrap",
        None,
        json!({ "username": "admin", "password": "correct horse battery staple" }),
    )
    .await;
    let cookie = cookie.unwrap();
    let (status, body, _) = call(
        &s.router,
        "POST",
        "/api/v1/tokens",
        Some(&cookie),
        json!({ "name": "socket", "permissions": permissions, "expires_in_days": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (body["secret"].as_str().unwrap().to_owned(), cookie)
}

async fn connect(
    s: &Served,
    secret: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::Error,
> {
    let mut request = format!("ws://{}/api/v1/events/socket", s.addr)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {secret}").parse().unwrap());
    tokio_tungstenite::connect_async(request)
        .await
        .map(|(ws, _)| ws)
}

fn change() -> ServerEvent {
    ServerEvent::ContainerChanged {
        change: ContainerChange {
            container_id: "abc".to_owned(),
            name: None,
            project: Some("blog".to_owned()),
            action: "start".to_owned(),
        },
    }
}

#[tokio::test]
async fn events_arrive_on_the_socket_as_json() {
    let s = serve().await;
    let (secret, _) = token(&s, &["host.view"]).await;
    let mut ws = connect(&s, &secret).await.expect("connects");

    // Published once the socket is surely subscribed.
    tokio::time::sleep(Duration::from_millis(100)).await;
    s.state.runner.publish(change());

    let message = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("an event within 5s")
        .expect("open")
        .expect("ok");
    let Message::Text(text) = message else {
        panic!("{message:?}")
    };
    let event: ServerEvent = serde_json::from_str(&text).unwrap();
    assert_eq!(event, change());
}

#[tokio::test]
async fn the_socket_needs_host_view() {
    let s = serve().await;
    let (secret, _) = token(&s, &["logs.view"]).await;
    let refused = connect(&s, &secret).await;
    assert!(
        refused.is_err(),
        "a token without host.view must not get events"
    );
}

#[tokio::test]
async fn revoking_the_token_closes_the_socket() {
    let s = serve().await;
    let (secret, cookie) = token(&s, &["host.view"]).await;
    let mut ws = connect(&s, &secret).await.expect("connects");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let (_, list, _) = call(
        &s.router,
        "GET",
        "/api/v1/tokens",
        Some(&cookie),
        Value::Null,
    )
    .await;
    let id = list[0]["id"].as_i64().unwrap();
    call(
        &s.router,
        "DELETE",
        &format!("/api/v1/tokens/{id}"),
        Some(&cookie),
        Value::Null,
    )
    .await;

    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(message)) = ws.next().await {
            if message.is_close() {
                return;
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the socket outlived its token");
}

async fn connect_with_cookie(
    s: &Served,
    cookie: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let mut request = format!("ws://{}/api/v1/events/socket", s.addr)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    tokio_tungstenite::connect_async(request)
        .await
        .expect("connects")
        .0
}

/// Whether the socket closes within `within`.
async fn closes(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    within: Duration,
) -> bool {
    tokio::time::timeout(within, async {
        while let Some(Ok(message)) = ws.next().await {
            if message.is_close() {
                return;
            }
        }
    })
    .await
    .is_ok()
}

#[tokio::test]
async fn signing_out_closes_that_sessions_socket_and_no_other() {
    let s = serve().await;
    let (_, signed_out) = token(&s, &["host.view"]).await;
    let (_, _, other) = call(
        &s.router,
        "POST",
        "/api/v1/auth/login",
        None,
        json!({ "username": "admin", "password": "correct horse battery staple" }),
    )
    .await;
    let other = other.unwrap();

    let mut leaving = connect_with_cookie(&s, &signed_out).await;
    let mut staying = connect_with_cookie(&s, &other).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let (status, _, _) = call(
        &s.router,
        "POST",
        "/api/v1/auth/logout",
        Some(&signed_out),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert!(
        closes(&mut leaving, Duration::from_secs(5)).await,
        "the socket outlived its session"
    );
    assert!(
        !closes(&mut staying, Duration::from_millis(500)).await,
        "signing out on one device closed another's socket"
    );
}

async fn connect_to(
    s: &Served,
    path: &str,
    secret: &str,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    let mut request = format!("ws://{}{path}", s.addr)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {secret}").parse().unwrap());
    tokio_tungstenite::connect_async(request).await.map(|_| ())
}

fn status_of(e: tokio_tungstenite::tungstenite::Error) -> u16 {
    match e {
        tokio_tungstenite::tungstenite::Error::Http(response) => response.status().as_u16(),
        other => panic!("expected an HTTP refusal, got {other}"),
    }
}

#[tokio::test]
async fn following_logs_over_a_socket_needs_logs_view_and_a_daemon() {
    let s = serve().await;
    let path = "/api/v1/hosts/1/containers/x/logs/socket";

    let (without, _) = token(&s, &["host.view"]).await;
    let refused = connect_to(&s, path, &without).await.expect_err("refused");
    assert_eq!(status_of(refused), 403);
}

#[tokio::test]
async fn following_logs_over_a_socket_says_when_there_is_no_daemon() {
    let s = serve().await;
    let (with, _) = token(&s, &["logs.view"]).await;
    let refused = connect_to(&s, "/api/v1/hosts/1/containers/x/logs/socket", &with)
        .await
        .expect_err("no daemon in tests");
    assert_eq!(status_of(refused), 503);
}

#[tokio::test]
async fn metrics_reach_only_sockets_that_asked_for_them() {
    let s = serve().await;
    let (secret, _) = token(&s, &["host.view"]).await;
    let mut ws = connect(&s, &secret).await.expect("connects");
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Not watching: a tick is skipped, and the next event is the change.
    s.state.sampler.tick(5);
    tokio::time::sleep(Duration::from_millis(50)).await;
    s.state.runner.publish(change());
    let first = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Text(text) = first else { panic!() };
    assert!(text.contains("container_changed"), "{text}");

    // Watching: ticks arrive.
    use futures::SinkExt as _;
    ws.send(Message::Text(r#"{"watch":"metrics"}"#.into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    s.state.sampler.tick(10);
    let next = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Text(text) = next else { panic!() };
    let event: ServerEvent = serde_json::from_str(&text).expect("an event the client reads");
    let ServerEvent::Metrics { now } = event else {
        panic!("{text}")
    };
    assert_eq!(now.at, 10, "the tick that came after asking, not before");
}
