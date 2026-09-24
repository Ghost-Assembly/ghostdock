//! One container's output, as a snapshot or followed live.
//!
//! Filtering happens in the browser over what was fetched. The alternative,
//! asking the daemon to search, would need a round trip per keystroke for a
//! body of text already in hand.

use std::sync::Arc;

use leptos::html::Pre;
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use shared::logs::{LogLine, Resume, Stream};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{CloseEvent, MessageEvent, WebSocket};

use crate::api;
use crate::screen::Screen;

/// Most lines kept on screen. Following a chatty container for an hour would
/// otherwise grow the page until the phone gives up.
const MAX_LINES: usize = 5_000;

/// How close to the bottom counts as "reading the latest", in pixels. Only
/// then does a new line scroll the view; someone who scrolled up to read
/// stays where they are.
const PINNED_WITHIN: i32 = 48;

/// Most lines in the page at once. The rest stay in memory, searchable and
/// downloadable; laying out thousands of rows on every new line is what
/// once starved a following log of the frames it needed to take input.
const ON_SCREEN: usize = 1_000;

/// A line as held here: shared rather than copied on every update, and
/// lower-cased once on arrival rather than on every keystroke of a search.
#[derive(Clone)]
struct Row {
    seq: u64,
    line: Arc<LogLine>,
    lower: Arc<str>,
}

/// An open follow socket. Dropping it closes the connection and frees the
/// callbacks the browser was holding, without reporting the close as an
/// ending: whoever dropped it already knows.
struct Follower {
    socket: WebSocket,
    _line: Closure<dyn FnMut(MessageEvent)>,
    _close: Closure<dyn FnMut(CloseEvent)>,
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.socket.set_onclose(None);
        let _ = self.socket.close();
    }
}

/// A WebSocket rather than an `EventSource`: a request held open takes one
/// of the six connections a browser allows per host over HTTP/1.1, shared
/// by every tab, and enough of them freeze the app.
fn follow_url(container: &str, since: Option<i64>) -> String {
    let location = web_sys::window().map(|w| w.location());
    let host = location
        .as_ref()
        .and_then(|l| l.host().ok())
        .unwrap_or_default();
    let secure = location
        .as_ref()
        .and_then(|l| l.protocol().ok())
        .is_some_and(|p| p == "https:");
    let since = since.map_or_else(String::new, |t| format!("?since={t}"));
    format!(
        "{}://{host}/api/v1/hosts/1/containers/{container}/logs/socket{since}",
        if secure { "wss" } else { "ws" }
    )
}

#[component]
pub fn ContainerLogs() -> impl IntoView {
    let params = use_params_map();
    let id = Memo::new(move |_| params.get().get("id").unwrap_or_default());

    // Each line carries a sequence number so the list can be keyed: a new
    // line then adds one node rather than redrawing thousands.
    let lines = RwSignal::new(Vec::<Row>::new());
    // Lines received since the last frame, applied together in the next one.
    let pending = StoredValue::new(Vec::<LogLine>::new());
    let flush_scheduled = StoredValue::new(false);
    let next_seq = StoredValue::new(0_u64);
    let loaded = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let note = RwSignal::new(None::<&'static str>);
    let filter = RwSignal::new(String::new());
    let errors_only = RwSignal::new(false);
    let following = RwSignal::new(false);
    let follower = StoredValue::new_local(None::<Follower>);
    let pane = NodeRef::<Pre>::new();
    let screen = Screen::new();

    let push = move |incoming: Vec<LogLine>| {
        let mut seq = next_seq.get_value();
        lines.update(|all| {
            for line in incoming {
                let lower: Arc<str> = line.text.to_lowercase().into();
                all.push(Row {
                    seq,
                    line: Arc::new(line),
                    lower,
                });
                seq += 1;
            }
            if all.len() > MAX_LINES {
                all.drain(..all.len() - MAX_LINES);
            }
        });
        next_seq.set_value(seq);
    };

    Effect::new(move |_| {
        let container = id.get();
        if container.is_empty() {
            return;
        }
        screen.load(async move {
            match api::container_logs(1, &container).await {
                Ok(fetched) => push(fetched.lines),
                Err(e) => error.set(Some(e.message)),
            }
            loaded.set(true);
        });
    });

    let stop = move |why: Option<&'static str>| {
        follower.set_value(None);
        following.set(false);
        note.set(why);
    };

    let start = move || {
        // Continue from what is already shown; see Resume for why the
        // overlap is matched line by line rather than by timestamp.
        let resume = lines.with_untracked(|all| Resume::after(all.iter().map(|row| &*row.line)));
        let url = follow_url(&id.get_untracked(), resume.since());
        let Ok(socket) = WebSocket::new(&url) else {
            note.set(Some("Could not start following."));
            return;
        };

        let on_line = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
            let Some(text) = ev.data().as_string() else {
                return;
            };
            let Ok(line) = serde_json::from_str::<LogLine>(&text) else {
                return;
            };
            if !resume.is_new(&line) {
                return;
            }
            pending.update_value(|batch| batch.push(line));
            if flush_scheduled.get_value() {
                return;
            }
            flush_scheduled.set_value(true);
            // One update per frame, however many lines arrive in it.
            screen.next_frame(move || {
                flush_scheduled.set_value(false);
                let batch = std::mem::take(&mut *pending.write_value());
                let pinned = pane.get_untracked().is_none_or(|el| {
                    el.scroll_top() + el.client_height() >= el.scroll_height() - PINNED_WITHIN
                });
                push(batch);
                if pinned {
                    screen.next_frame(move || {
                        if let Some(el) = pane.get_untracked() {
                            el.set_scroll_top(el.scroll_height());
                        }
                    });
                }
            });
        });
        // Not reconnected automatically: a reconnect would replay lines.
        let on_close = Closure::<dyn FnMut(CloseEvent)>::new(move |ev: CloseEvent| {
            stop(Some(match ev.reason().as_str() {
                "stopped" => "The container stopped, so there is nothing more to follow.",
                "revoked" => "Access to these logs was withdrawn.",
                _ => "Following stopped. Tap Follow to pick up again.",
            }));
        });
        socket.set_onmessage(Some(on_line.as_ref().unchecked_ref()));
        socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));

        follower.set_value(Some(Follower {
            socket,
            _line: on_line,
            _close: on_close,
        }));
        following.set(true);
        note.set(None);
    };

    on_cleanup(move || follower.set_value(None));

    // Which lines match, by sequence number: cheap to compute from shared
    // rows, and cheap to compare, so nothing redraws when nothing changed.
    let matching = Memo::new(move |_| {
        let needle = filter.get().to_lowercase();
        let only_errors = errors_only.get();
        lines.with(|all| {
            all.iter()
                .filter(|row| !only_errors || row.line.stream == Stream::Stderr)
                .filter(|row| needle.is_empty() || row.lower.contains(&needle))
                .map(|row| row.seq)
                .collect::<Vec<_>>()
        })
    });
    // The latest of them, as many as the page holds.
    let shown = Memo::new(move |_| {
        matching.with(|all| {
            let skip = all.len().saturating_sub(ON_SCREEN);
            all.iter().skip(skip).copied().collect::<Vec<_>>()
        })
    });
    let row_for = move |seq: u64| {
        lines.with_untracked(|all| {
            let first = all.first().map_or(0, |row| row.seq);
            usize::try_from(seq.saturating_sub(first))
                .ok()
                .and_then(|i| all.get(i))
                .map(|row| Arc::clone(&row.line))
        })
    };

    view! {
        <header class="topbar">
            <h1 class="wordmark">"Logs"</h1>
            <a class="topbar-link" href="/">"Back"</a>
        </header>

        <Show when=move || error.get().is_some()>
            <p class="notice" role="alert">{move || error.get().unwrap_or_default()}</p>
        </Show>

        <label class="field">
            <span class="field-label">"Search"</span>
            <input
                class="field-input"
                type="search"
                autocapitalize="none"
                spellcheck="false"
                placeholder="error, timeout, 500"
                prop:value=move || filter.get()
                on:input=move |ev| filter.set(event_target_value(&ev))
            />
        </label>

        <div class="actions actions-pair">
            <button
                class="button button-quiet"
                type="button"
                aria-pressed=move || following.get().to_string()
                on:click=move |_| if following.get_untracked() { stop(None) } else { start() }
            >
                {move || if following.get() { "Following" } else { "Follow" }}
            </button>
            <button
                class="button button-quiet"
                type="button"
                aria-pressed=move || errors_only.get().to_string()
                on:click=move |_| errors_only.update(|on| *on = !*on)
            >
                {move || if errors_only.get() { "Showing errors" } else { "Errors only" }}
            </button>
        </div>
        // A plain link, so the browser does the saving.
        <a
            class="button button-quiet"
            href=move || format!("/api/v1/hosts/1/containers/{}/logs.txt", id.get())
            download=move || format!("{}.log", shared::short(&id.get(), 12))
        >
            "Download"
        </a>

        <Show when=move || note.get().is_some()>
            <p class="entry-note">{move || note.get().unwrap_or_default()}</p>
        </Show>

        {move || {
            if !loaded.get() {
                return view! { <p class="state-note">"Reading output"</p> }.into_any();
            }
            let count = matching.with(Vec::len);
            if count == 0 && !following.get() {
                return view! {
                    <div class="state-note">
                        <p>"Nothing matches."</p>
                        <p>"This container may simply not have written anything."</p>
                    </div>
                }
                .into_any();
            }
            let lines_word = if count == 1 { "line" } else { "lines" };
            let summary = if count > ON_SCREEN {
                format!("The latest {ON_SCREEN} of {count} {lines_word}. Search looks through all of them.")
            } else {
                format!("{count} {lines_word}")
            };
            view! { <p class="verdict-count">{summary}</p> }.into_any()
        }}

        // Only when there is something in it, or something about to be.
        <Show when=move || loaded.get() && (following.get() || matching.with(|m| !m.is_empty()))>
            <pre class="log log-tall" node_ref=pane>
                <For each=move || shown.get() key=|seq| *seq let:seq>
                    {
                        row_for(seq).map(|line| {
                            let class = if line.stream == Stream::Stderr {
                                "log-line log-stderr"
                            } else {
                                "log-line"
                            };
                            view! { <div class=class>{line.text.clone()}"\n"</div> }
                        })
                    }
                </For>
            </pre>
        </Show>
    }
}
