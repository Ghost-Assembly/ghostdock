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

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use leptos::prelude::*;
use shared::event::ServerEvent;
use web_sys::MessageEvent;

use crate::socket;

// `Send + Sync` because Leptos's lifecycle hooks require it in order to
// support server-side rendering. Wasm is single threaded, so the Mutex never
// contends; it is the price of using the same API as everything else.
type Handler = Arc<dyn Fn(&ServerEvent) + Send + Sync>;
type Reconnect = Arc<dyn Fn() + Send + Sync>;

/// The shared event stream.
#[derive(Clone)]
pub struct Events {
    handlers: Arc<Mutex<Vec<(u64, Handler)>>>,
    reconnect: Arc<Mutex<Vec<(u64, Reconnect)>>>,
    next_id: Arc<AtomicU64>,
    /// True while the connection is down and being retried.
    paused: RwSignal<bool>,
}

impl Events {
    fn new() -> Self {
        Self {
            handlers: Arc::new(Mutex::new(Vec::new())),
            reconnect: Arc::new(Mutex::new(Vec::new())),
            next_id: Arc::new(AtomicU64::new(0)),
            paused: RwSignal::new(false),
        }
    }

    /// Whether live updates have stopped arriving for now. Whatever is on
    /// screen may be out of date until they resume.
    #[must_use]
    pub fn paused(&self) -> ReadSignal<bool> {
        self.paused.read_only()
    }

    /// Calls `handler` for every event, until the calling view is disposed.
    /// Lent rather than given: every subscriber gets the same event, and a
    /// copy each of the 5 s figures would be a deep clone per subscriber.
    pub fn on(&self, handler: impl Fn(&ServerEvent) + Send + Sync + 'static) {
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
            handler(event);
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
pub fn provide(path: &str) -> Events {
    let events = Events::new();
    let generation = GENERATION.with(|g| {
        let next = g.get() + 1;
        g.set(next);
        next
    });
    let link = Link {
        events: events.clone(),
        url: socket::url(path),
        generation,
    };
    connect(link.clone(), 0, false);

    // A phone puts a page in the background to sleep and drops its
    // connections; a laptop loses its network. The moment either is usable
    // again is the moment to reconnect, not whenever the backoff comes round.
    let online_link = link.clone();
    let online = window_event_listener(leptos::ev::online, move |_| kick(&online_link));
    let visible = window_event_listener_untyped("visibilitychange", move |_| {
        let hidden = web_sys::window()
            .and_then(|w| w.document())
            .is_none_or(|d| d.hidden());
        if !hidden {
            kick(&link);
        }
    });

    on_cleanup(move || {
        online.remove();
        visible.remove();
        // Signing out ends this bus: stop reconnecting and close.
        GENERATION.with(|g| {
            if g.get() == generation {
                g.set(generation + 1);
            }
        });
        WAITING.with(|w| w.set(None));
        SOCKET.with(|s| s.borrow_mut().take());
    });
    provide_context(events.clone());
    events
}

thread_local! {
    /// Bumped to stop an earlier bus's reconnect loop.
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    /// The open socket and the callbacks the browser holds for it. Kept
    /// here, not leaked, so a reconnect closes and frees the previous one.
    static SOCKET: RefCell<Option<socket::Owned>> = const { RefCell::new(None) };
    /// Views currently showing live figures.
    static WATCHERS: Cell<u32> = const { Cell::new(0) };
    /// The retry now scheduled, if any. Only that one may run, so
    /// reconnecting early turns the timer already set into a no-op.
    static WAITING: Cell<Option<u64>> = const { Cell::new(None) };
    static NEXT_RETRY: Cell<u64> = const { Cell::new(0) };
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
        if let Some(socket) = s.borrow().as_ref() {
            socket.send(text);
        }
    });
}

/// Where to connect, and for which bus.
#[derive(Clone)]
struct Link {
    events: Events,
    url: String,
    generation: u64,
}

const FIRST_RETRY_MS: u32 = 1_000;
const LONGEST_RETRY_MS: u32 = 30_000;

fn connect(link: Link, attempt: u32, was_open: bool) {
    if GENERATION.with(Cell::get) != link.generation {
        return;
    }
    WAITING.with(|w| w.set(None));
    let Ok(socket) = socket::Owned::connect(&link.url) else {
        retry(link, attempt, was_open);
        return;
    };

    let dispatcher = link.events.clone();
    let opened = Rc::new(Cell::new(false));
    let (now_open, events) = (Rc::clone(&opened), link.events.clone());
    let socket = socket
        .on_message(move |ev: MessageEvent| {
            let Some(text) = ev.data().as_string() else {
                return;
            };
            match serde_json::from_str::<ServerEvent>(&text) {
                Ok(event) => dispatcher.dispatch(&event),
                // A server and client from different builds; not a reason
                // to drop the connection.
                Err(e) => leptos::logging::warn!("unreadable server event: {e}"),
            }
        })
        .on_open(move || {
            now_open.set(true);
            events.paused.set(false);
            // Events during the gap are not replayed: views reload instead.
            if was_open {
                events.reconnected();
            }
            // The server forgets what a closed socket watched.
            if WATCHERS.with(Cell::get) > 0 {
                send_watch(true);
            }
        })
        .on_close(move |_| {
            link.events.paused.set(true);
            // A connection that worked starts the backoff over: one drop
            // after a day connected should not wait as long as the tenth
            // failure in a row.
            let attempt = if opened.get() { 0 } else { attempt };
            retry(link.clone(), attempt, true);
        });

    // Replacing the previous connection closes it and drops its callbacks.
    // This never runs inside one of them: reconnects are scheduled, or come
    // from a window event, and are never called from a socket's handler.
    SOCKET.with(|s| *s.borrow_mut() = Some(socket));
}

// The bus belongs to the signed-in app, not to a screen, so its timer is not
// a Screen's; the generation check in `connect` is what stops it on sign-out.
#[allow(clippy::disallowed_methods)]
fn retry(link: Link, attempt: u32, was_open: bool) {
    let delay = FIRST_RETRY_MS
        .saturating_mul(1 << attempt.min(5))
        .min(LONGEST_RETRY_MS);
    let ticket = NEXT_RETRY.with(|n| {
        let next = n.get() + 1;
        n.set(next);
        next
    });
    WAITING.with(|w| w.set(Some(ticket)));
    set_timeout(
        move || {
            if WAITING.with(Cell::get) == Some(ticket) {
                connect(link, attempt + 1, was_open);
            }
        },
        std::time::Duration::from_millis(u64::from(delay)),
    );
}

/// Reconnects at once if the bus is waiting to retry. An open socket, or
/// one still connecting, is left alone.
fn kick(link: &Link) {
    if WAITING.with(Cell::get).is_some() {
        connect(link.clone(), 0, true);
    }
}

/// The shared event stream, if one was provided.
#[must_use]
pub fn use_events() -> Option<Events> {
    use_context::<Events>()
}
