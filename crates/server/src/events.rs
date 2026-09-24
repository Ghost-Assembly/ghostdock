//! Live events, one stream carrying every kind.
//!
//! Offered two ways with the same content and rules. The browser uses a
//! WebSocket: over plain HTTP/1.1 a browser allows six connections per host
//! across all tabs, and an event stream per tab held one of them forever, so
//! the sixth tab's requests queued behind the others and the app froze.
//! WebSockets do not count against that limit. Server-sent events remain for
//! API clients, where they are the simpler thing to consume.

use std::convert::Infallible;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use futures::stream::{Stream, StreamExt};
use tokio_stream::wrappers::BroadcastStream;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use shared::event::ServerEvent;

use crate::auth::{Authorized, perm};
use crate::origin::SameOrigin;
use crate::state::AppState;

/// Interval for keep-alive comments.
///
/// Proxies commonly drop an idle connection after a minute, and a deploy can
/// be quiet for far longer than that while an image downloads.
const KEEP_ALIVE: Duration = Duration::from_secs(20);

/// How often the socket is pinged, for the same reason as [`KEEP_ALIVE`].
const PING: Duration = Duration::from_secs(20);

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/events", get(stream))
        .route("/events/socket", get(socket))
}

async fn socket(
    principal: Authorized<perm::HostView>,
    // Before the upgrade, as for the shell: a page on another origin can
    // open a WebSocket here and the browser would attach the cookie.
    _origin: SameOrigin,
    State(state): State<AppState>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let revoked = state.revocations.until_revoked(&principal);
    let events = state.runner.subscribe();
    upgrade.on_upgrade(move |socket| pump(socket, events, revoked))
}

async fn pump(
    socket: WebSocket,
    mut events: tokio::sync::broadcast::Receiver<ServerEvent>,
    revoked: impl std::future::Future<Output = ()> + Send + 'static,
) {
    use futures::SinkExt as _;
    use tokio::sync::broadcast::error::RecvError;

    let (mut sink, mut incoming) = socket.split();
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING, PING);
    // Resource figures come every 5 s and are large; only a screen showing
    // them asks for them.
    let mut watching = false;
    tokio::pin!(revoked);
    loop {
        tokio::select! {
            () = &mut revoked => {
                let _ = sink.send(Message::Close(None)).await;
                return;
            }
            received = events.recv() => match received {
                Ok(event) => {
                    if matches!(event, ServerEvent::Metrics { .. }) && !watching {
                        continue;
                    }
                    let Ok(json) = serde_json::to_string(&event) else { continue };
                    if sink.send(Message::Text(json.into())).await.is_err() {
                        return;
                    }
                }
                // Missed events are not replayed; the client reloads what it
                // shows when it reconnects, which a lag is not worth forcing.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            },
            _ = ping.tick() => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return;
                }
            }
            message = incoming.next() => match message {
                // The only thing a client says: whether it is showing figures.
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage { watch: Some(Topic::Metrics), .. }) => watching = true,
                    Ok(ClientMessage { unwatch: Some(Topic::Metrics), .. }) => watching = false,
                    _ => {}
                },
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            },
        }
    }
}

/// `{"watch":"metrics"}` or `{"unwatch":"metrics"}`.
#[derive(Debug, serde::Deserialize)]
struct ClientMessage {
    watch: Option<Topic>,
    unwatch: Option<Topic>,
}

#[derive(Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum Topic {
    Metrics,
}

async fn stream(
    principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let revoked = state.revocations.until_revoked(&principal);
    let events = BroadcastStream::new(state.runner.subscribe()).filter_map(|received| async move {
        // A receiver that falls behind is told it lagged rather than being
        // silently fed a gap; dropping those keeps the stream well-formed.
        let event = received.ok()?;
        // Figures go only to sockets that ask; an SSE client cannot ask.
        if matches!(event, ServerEvent::Metrics { .. }) {
            return None;
        }
        Event::default()
            .event(event.name())
            .json_data(&event)
            .ok()
            .map(Ok)
    });
    // Ends the response, not just the events: keep-alives stop too.
    let events = events.take_until(revoked);

    Sse::new(events).keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
}
