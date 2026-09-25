//! Live events, one stream carrying every kind.
//!
//! Offered two ways with the same content and rules. The browser uses a
//! WebSocket: over plain HTTP/1.1 a browser allows six connections per host
//! across all tabs, and an event stream per tab held one of them forever, so
//! the sixth tab's requests queued behind the others and the app froze.
//! WebSockets do not count against that limit. Server-sent events remain for
//! API clients, where they are the simpler thing to consume.

use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::{Stream, StreamExt};
use shared::reference::Access;
use shared::token::Permission;
use tokio::sync::{broadcast, watch};
use tokio_stream::wrappers::BroadcastStream;

use axum::extract::ws::{Message, Utf8Bytes, WebSocketUpgrade};
use axum::response::Response;
use shared::event::ServerEvent;

use crate::auth::{Authorized, perm};
use crate::origin::SameOrigin;
use crate::reference::Routes;
use crate::socket::{Next, Source};
use crate::state::AppState;

/// Interval for keep-alive comments, for the same reason a socket is
/// pinged.
const KEEP_ALIVE: std::time::Duration = crate::socket::PING;

pub fn routes() -> Routes {
    let view = Access::Token(Permission::HostView);
    Routes::new("Events")
        .get(
            "/events",
            view,
            "Every server event as server-sent events, for API clients",
            stream,
        )
        .get(
            "/events/socket",
            view,
            "The same events over a WebSocket, as the browser gets them; send {\"watch\":\"metrics\"} for resource figures",
            socket,
        )
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
    let events = Events {
        events: state.runner.subscribe(),
        ticks: state.sampler.ticks(),
        watching: false,
    };
    upgrade.on_upgrade(move |socket| crate::socket::pump(socket, events, revoked))
}

/// Every event, and resource figures while the client asks for them.
struct Events {
    events: broadcast::Receiver<ServerEvent>,
    /// Each tick, already written once for every socket; sent as it is.
    ticks: watch::Receiver<Option<Utf8Bytes>>,
    /// Figures come every 5 s and are large; only a screen showing them
    /// asks for them.
    watching: bool,
}

impl Source for Events {
    async fn next(&mut self) -> Next {
        use broadcast::error::RecvError;
        loop {
            tokio::select! {
                changed = self.ticks.changed(), if self.watching => {
                    match changed.ok().and_then(|()| self.ticks.borrow_and_update().clone()) {
                        Some(json) => return Next::Send(Message::Text(json)),
                        // The sampler is gone; nothing more will come.
                        None => self.watching = false,
                    }
                }
                received = self.events.recv() => match received {
                    Ok(event) => {
                        if let Ok(json) = serde_json::to_string(&event) {
                            return Next::Send(Message::Text(json.into()));
                        }
                    }
                    // Missed events are not replayed; the client reloads
                    // what it shows when it reconnects, which a lag is not
                    // worth forcing.
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return Next::End(None),
                },
            }
        }
    }

    /// The only thing a client says: whether it is showing figures.
    fn heard(&mut self, text: &str) {
        match serde_json::from_str::<ClientMessage>(text) {
            Ok(ClientMessage {
                watch: Some(Topic::Metrics),
                ..
            }) => {
                // From the next tick on, as before a socket watched.
                self.ticks.mark_unchanged();
                self.watching = true;
            }
            Ok(ClientMessage {
                unwatch: Some(Topic::Metrics),
                ..
            }) => self.watching = false,
            _ => {}
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
        // Figures never come this way: they go only to sockets that ask,
        // and an SSE client cannot ask.
        let event = received.ok()?;
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
