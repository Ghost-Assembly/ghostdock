//! A shell inside a container.
//!
//! A scrollback and an input box rather than a terminal emulator. The shell
//! runs without a TTY, so what arrives is plain lines; there is nothing to
//! emulate, and nothing here pretends otherwise. Cursor-addressed programs
//! will not work, which is the honest trade for a screen that is usable with
//! a soft keyboard.

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use shared::logs::{LogLine, Stream};
use web_sys::MessageEvent;

use crate::{api, socket};

/// Lines kept on screen.
///
/// A command like `find /` would otherwise grow the page until the phone
/// gives up.
const MAX_LINES: usize = 1_000;

#[component]
pub fn Console() -> impl IntoView {
    let params = use_params_map();
    let id = Memo::new(move |_| params.get().get("id").unwrap_or_default());

    let lines = RwSignal::new(Vec::<LogLine>::new());
    let command = RwSignal::new(String::new());
    let connected = RwSignal::new(false);
    // The shell is this screen's: it closes when the screen goes, and when
    // the route moves to another container. Left open, it would keep a
    // shell running on the server that nobody can see.
    let shell = StoredValue::new_local(None::<socket::Owned>);
    on_cleanup(move || shell.set_value(None));

    Effect::new(move |_| {
        let container = id.get();
        // Whatever the last container said is not this one's.
        shell.set_value(None);
        connected.set(false);
        lines.set(Vec::new());
        if container.is_empty() {
            return;
        }

        let url = socket::url(&format!(
            "/api/v1/hosts/1/containers/{}/exec",
            api::component(&container)
        ));
        match socket::Owned::connect(&url) {
            Ok(opened) => shell.set_value(Some(
                opened
                    .on_message(move |ev: MessageEvent| {
                        let Some(text) = ev.data().as_string() else {
                            return;
                        };
                        if let Ok(line) = serde_json::from_str::<LogLine>(&text) {
                            lines.update(|all| {
                                all.push(line);
                                if all.len() > MAX_LINES {
                                    all.remove(0);
                                }
                            });
                        }
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
            lines.update(|all| {
                all.push(LogLine {
                    stream: Stream::Stdout,
                    at: None,
                    text: format!("$ {text}"),
                });
            });
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

        <pre class="log log-tall">
            {move || {
                lines
                    .get()
                    .into_iter()
                    .map(|line| {
                        let class = if line.stream == Stream::Stderr {
                            "log-line log-stderr"
                        } else {
                            "log-line"
                        };
                        view! { <div class=class>{line.text}"\n"</div> }
                    })
                    .collect_view()
            }}
        </pre>

        <form on:submit=submit>
            <label class="field">
                <span class="field-label">"Command"</span>
                <input
                    class="field-input field-mono"
                    type="text"
                    autocapitalize="none"
                    spellcheck="false"
                    placeholder="ls -la"
                    prop:value=move || command.get()
                    on:input=move |ev| command.set(event_target_value(&ev))
                />
            </label>
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
