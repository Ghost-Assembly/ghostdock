//! A shell inside a container.
//!
//! A scrollback and an input box rather than a terminal emulator. The shell
//! runs without a TTY, so what arrives is plain lines; there is nothing to
//! emulate, and nothing here pretends otherwise. Cursor-addressed programs
//! will not work, which is the honest trade for a screen that is usable with
//! a soft keyboard.

use std::collections::VecDeque;
use std::sync::Arc;

use leptos::html::Pre;
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use shared::logs::{LogLine, Stream};
use web_sys::MessageEvent;

use crate::screen::Screen;
use crate::ui::{Field, OutputLine, pinned, to_bottom};
use crate::{api, socket};

/// Lines kept on screen.
///
/// A command like `find /` would otherwise grow the page until the phone
/// gives up.
const MAX_LINES: usize = 1_000;

/// A line as held here: numbered on arrival, so the scrollback is keyed and
/// a new line adds one node rather than redrawing the rest, and shared, so
/// reading the list does not copy the text.
#[derive(Clone)]
struct Row {
    seq: u64,
    line: Arc<LogLine>,
}

#[component]
pub fn Console() -> impl IntoView {
    let params = use_params_map();
    let id = Memo::new(move |_| params.get().get("id").unwrap_or_default());

    let lines = RwSignal::new(VecDeque::<Row>::new());
    let next_seq = StoredValue::new(0_u64);
    // Lines received since the last frame, applied together in the next one.
    let pending = StoredValue::new(Vec::<LogLine>::new());
    let flush_scheduled = StoredValue::new(false);
    let command = RwSignal::new(String::new());
    let connected = RwSignal::new(false);
    let pane = NodeRef::<Pre>::new();
    let screen = Screen::new();
    // The shell is this screen's: it closes when the screen goes, and when
    // the route moves to another container. Left open, it would keep a
    // shell running on the server that nobody can see.
    let shell = StoredValue::new_local(None::<socket::Owned>);
    on_cleanup(move || shell.set_value(None));

    let push = move |incoming: Vec<LogLine>| {
        let mut seq = next_seq.get_value();
        lines.update(|all| {
            for line in incoming {
                all.push_back(Row {
                    seq,
                    line: Arc::new(line),
                });
                seq += 1;
            }
            let over = all.len().saturating_sub(MAX_LINES);
            all.drain(..over);
        });
        next_seq.set_value(seq);
    };
    // Adds lines, and follows them down if the reader was at the bottom.
    let show = move |incoming: Vec<LogLine>| {
        let pinned = pinned(pane);
        push(incoming);
        if pinned {
            screen.next_frame(move || to_bottom(pane));
        }
    };

    Effect::new(move |_| {
        let container = id.get();
        // Whatever the last container said is not this one's.
        shell.set_value(None);
        connected.set(false);
        pending.set_value(Vec::new());
        lines.update(VecDeque::clear);
        if container.is_empty() {
            return;
        }

        let url = socket::url(&format!(
            "/api/v1/hosts/{}/containers/{}/exec",
            api::HOST,
            api::component(&container)
        ));
        match socket::Owned::connect(&url) {
            Ok(opened) => shell.set_value(Some(
                opened
                    .on_message(move |ev: MessageEvent| {
                        let Some(text) = ev.data().as_string() else {
                            return;
                        };
                        let Ok(line) = serde_json::from_str::<LogLine>(&text) else {
                            return;
                        };
                        pending.update_value(|batch| batch.push(line));
                        if flush_scheduled.get_value() {
                            return;
                        }
                        flush_scheduled.set_value(true);
                        // One update per frame, however many lines arrive in it.
                        screen.next_frame(move || {
                            flush_scheduled.set_value(false);
                            show(std::mem::take(&mut *pending.write_value()));
                        });
                    })
                    .on_open(move || connected.set(true))
                    .on_close(move |_| connected.set(false)),
            )),
            Err(e) => leptos::logging::error!("could not open a shell: {e:?}"),
        }
    });

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let text = command.get();
        if text.is_empty() {
            return;
        }
        let sent = shell.with_value(|s| s.as_ref().is_some_and(|s| s.send(&text)));
        if sent {
            // Echoed locally: without a TTY the shell does not echo, so
            // without this the scrollback shows answers with no questions.
            // After anything still waiting for a frame, which came first.
            let mut batch = std::mem::take(&mut *pending.write_value());
            batch.push(LogLine {
                stream: Stream::Stdout,
                at: None,
                text: format!("$ {text}"),
            });
            show(batch);
            command.set(String::new());
        }
    };

    view! {
        <header class="topbar">
            <h1 class="wordmark">"Shell"</h1>
            <a class="topbar-link" href="/">"Back"</a>
        </header>

        <p class="entry-note">
            {move || {
                if connected.get() {
                    "Connected. Type a command and press enter."
                } else {
                    "Not connected. The container must be running."
                }
            }}
        </p>

        <pre class="log log-tall" node_ref=pane>
            <For each=move || lines.get() key=|row| row.seq let:row>
                <OutputLine text=row.line.text.clone() stderr=row.line.stream == Stream::Stderr />
            </For>
        </pre>

        <form on:submit=submit>
            <Field label="Command">
                <input
                    class="field-input field-mono"
                    type="text"
                    autocapitalize="none"
                    spellcheck="false"
                    placeholder="ls -la"
                    prop:value=move || command.get()
                    on:input=move |ev| command.set(event_target_value(&ev))
                />
            </Field>
            <button class="button" type="submit" disabled=move || !connected.get()>
                "Run"
            </button>
            <p class="entry-note">
                "A plain shell without a terminal, so interactive programs such as \
                 vi and top will not work here."
            </p>
        </form>
    }
}
