//! Live updates from the server.
//!
//! One WebSocket for the whole application, shared through context. See
//! [`provide`] for why a socket and not an `EventSource`.
//!
//! Delivery is by callback, deliberately, rather than through a signal
//! holding the latest event. Reactive effects coalesce: a burst of output
//! lines would overwrite the slot before an effect ran, and a view would see
//! only the last one. Signals model current state; this is a stream, and
//! every message has to arrive.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use leptos::prelude::*;
use shared::event::ServerEvent;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{MessageEvent, WebSocket};

// `Send + Sync` because Leptos's lifecycle hooks require it in order to
// support server-side rendering. Wasm is single threaded, so the Mutex never
// contends; it is the price of using the same API as everything else.
type Handler = Arc<dyn Fn(ServerEvent) + Send + Sync>;
type Reconnect = Arc<dyn Fn() + Send + Sync>;

/// The shared event stream.
#[derive(Clone)]
pub struct Events {
    handlers: Arc<Mutex<Vec<(u64, Handler)>>>,
    reconnect: Arc<Mutex<Vec<(u64, Reconnect)>>>,
    next_id: Arc<AtomicU64>,
}

impl Events {
    fn new() -> Self {
        Self {
            handlers: Arc::new(Mutex::new(Vec::new())),
            reconnect: Arc::new(Mutex::new(Vec::new())),
            next_id: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Calls `handler` for every event, until the calling view is disposed.
    pub fn on(&self, handler: impl Fn(ServerEvent) + Send + Sync + 'static) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.lock().push((id, Arc::new(handler)));

        // A view that goes away must stop being called, or its closure keeps
        // writing to signals nothing renders any more.
        let handlers = Arc::clone(&self.handlers);
        on_cleanup(move || {
            if let Ok(mut handlers) = handlers.lock() {
                handlers.retain(|(existing, _)| *existing != id);
            }
        });
    }

    /// Asks for the 5 s figures while the calling view exists. Screens
    /// without figures never ask, so a phone on Settings gets nothing extra.
    pub fn watch_metrics(&self) {
        WATCHERS.with(|w| w.set(w.get() + 1));
        send_watch(true);
        on_cleanup(|| {
            let left = WATCHERS.with(|w| {
                w.set(w.get().saturating_sub(1));
                w.get()
            });
            if left == 0 {
                send_watch(false);
            }
        });
    }

    /// Calls `handler` after the connection comes back, until the calling
    /// view is disposed. Whatever happened while it was down was missed, so
    /// a view showing live state should reload it here.
    pub fn on_reconnect(&self, handler: impl Fn() + Send + Sync + 'static) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut list) = self.reconnect.lock() {
            list.push((id, Arc::new(handler)));
        }
        let reconnect = Arc::clone(&self.reconnect);
        on_cleanup(move || {
            if let Ok(mut list) = reconnect.lock() {
                list.retain(|(existing, _)| *existing != id);
            }
        });
    }

    fn reconnected(&self) {
        let snapshot: Vec<_> = self
            .reconnect
            .lock()
            .map(|list| list.iter().map(|(_, h)| Arc::clone(h)).collect())
            .unwrap_or_default();
        for handler in snapshot {
            handler();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(u64, Handler)>> {
        self.handlers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn dispatch(&self, event: &ServerEvent) {
        // Snapshot first: a handler may subscribe or unsubscribe while being
        // called, and mutating the list mid-iteration would panic on the
        // outstanding borrow.
        let snapshot: Vec<Handler> = self
            .lock()
            .iter()
            .map(|(_, handler)| Arc::clone(handler))
            .collect();

        for handler in snapshot {
            handler(event.clone());
        }
    }
}

/// Opens the socket and provides the bus through context, until the
/// calling view (the signed-in shell) goes away.
///
/// A WebSocket rather than an `EventSource`: over plain HTTP/1.1 a browser
/// allows six connections per host across every tab, and an event stream
/// per tab held one each for good, so a sixth tab froze waiting for a slot.
/// WebSockets do not count against that limit. Unlike an `EventSource` they
/// do not reconnect by themselves, so that is done here, with backoff.
pub fn provide(path: &str) {
    let events = Events::new();
    let generation = GENERATION.with(|g| {
        let next = g.get() + 1;
        g.set(next);
        next
    });
    connect(events.clone(), socket_url(path), generation, 0, false);
    on_cleanup(move || {
        // Signing out ends this bus: stop reconnecting and close.
        GENERATION.with(|g| {
            if g.get() == generation {
                g.set(generation + 1);
            }
        });
        SOCKET.with(|s| s.borrow_mut().take());
    });
    provide_context(events);
}

thread_local! {
    /// Bumped to stop an earlier bus's reconnect loop.
    static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The open socket and the callbacks the browser holds for it. Kept
    /// here, not leaked, so a reconnect frees the previous connection's.
    static SOCKET: std::cell::RefCell<Option<Connection>> = const { std::cell::RefCell::new(None) };
    /// Views currently showing live figures.
    static WATCHERS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Tells the server whether to send figures. A socket still connecting
/// drops this; opening re-sends it.
fn send_watch(on: bool) {
    let text = if on {
        r#"{"watch":"metrics"}"#
    } else {
        r#"{"unwatch":"metrics"}"#
    };
    SOCKET.with(|s| {
        if let Some(c) = s.borrow().as_ref() {
            let _ = c.socket.send_with_str(text);
        }
    });
}

struct Connection {
    socket: WebSocket,
    _message: Closure<dyn FnMut(MessageEvent)>,
    _open: Closure<dyn FnMut()>,
    _close: Closure<dyn FnMut()>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.socket.set_onclose(None);
        let _ = self.socket.close();
    }
}

const FIRST_RETRY_MS: u32 = 1_000;
const LONGEST_RETRY_MS: u32 = 30_000;

fn socket_url(path: &str) -> String {
    let location = web_sys::window().map(|w| w.location());
    let host = location
        .as_ref()
        .and_then(|l| l.host().ok())
        .unwrap_or_default();
    let secure = location
        .as_ref()
        .and_then(|l| l.protocol().ok())
        .is_some_and(|p| p == "https:");
    format!("{}://{host}{path}", if secure { "wss" } else { "ws" })
}

fn connect(events: Events, url: String, generation: u64, attempt: u32, was_open: bool) {
    if GENERATION.with(std::cell::Cell::get) != generation {
        return;
    }
    let Ok(socket) = WebSocket::new(&url) else {
        retry(events, url, generation, attempt, was_open);
        return;
    };

    let dispatcher = events.clone();
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
        let Some(text) = ev.data().as_string() else {
            return;
        };
        match serde_json::from_str::<ServerEvent>(&text) {
            Ok(event) => dispatcher.dispatch(&event),
            // A server and client from different builds; not a reason to
            // drop the connection.
            Err(e) => leptos::logging::warn!("unreadable server event: {e}"),
        }
    });
    let reconnected = events.clone();
    let on_open = Closure::<dyn FnMut()>::new(move || {
        // Events during the gap are not replayed: views reload instead.
        if was_open {
            reconnected.reconnected();
        }
        // The server forgets what a closed socket watched.
        if WATCHERS.with(std::cell::Cell::get) > 0 {
            send_watch(true);
        }
    });
    let (again, again_url) = (events, url);
    let on_close = Closure::<dyn FnMut()>::new(move || {
        retry(again.clone(), again_url.clone(), generation, attempt, true);
    });
    socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));

    // Replacing the previous connection drops its callbacks. This never runs
    // inside one of them: reconnects are scheduled, not called directly.
    SOCKET.with(|s| {
        *s.borrow_mut() = Some(Connection {
            socket,
            _message: on_message,
            _open: on_open,
            _close: on_close,
        });
    });
}

// The bus belongs to the signed-in app, not to a screen, so its timer is not
// a Screen's; the generation check above is what stops it on sign-out.
#[allow(clippy::disallowed_methods)]
fn retry(events: Events, url: String, generation: u64, attempt: u32, was_open: bool) {
    let delay = FIRST_RETRY_MS
        .saturating_mul(1 << attempt.min(5))
        .min(LONGEST_RETRY_MS);
    set_timeout(
        move || connect(events, url, generation, attempt + 1, was_open),
        std::time::Duration::from_millis(u64::from(delay)),
    );
}

/// The shared event stream, if one was provided.
#[must_use]
pub fn use_events() -> Option<Events> {
    use_context::<Events>()
}
