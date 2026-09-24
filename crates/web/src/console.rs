//! A shell inside a container.
//!
//! A scrollback and an input box rather than a terminal emulator. The shell
//! runs without a TTY, so what arrives is plain lines; there is nothing to
//! emulate, and nothing here pretends otherwise. Cursor-addressed programs
//! will not work, which is the honest trade for a screen that is usable with
//! a soft keyboard.

use std::sync::{Arc, Mutex};

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use shared::logs::{LogLine, Stream};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{MessageEvent, WebSocket};

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
    // Held so the socket outlives this function and can be written to.
    let socket: Arc<Mutex<Option<WebSocket>>> = Arc::new(Mutex::new(None));

    let held = Arc::clone(&socket);
    Effect::new(move |_| {
        let container = id.get();
        if container.is_empty() {
            return;
        }

        let location = web_sys::window().and_then(|w| w.location().host().ok());
        let Some(host) = location else { return };
        let secure = web_sys::window()
            .and_then(|w| w.location().protocol().ok())
            .is_some_and(|p| p == "https:");
        let scheme = if secure { "wss" } else { "ws" };
        let url = format!("{scheme}://{host}/api/v1/hosts/1/containers/{container}/exec");

        match WebSocket::new(&url) {
            Ok(ws) => {
                let on_message =
                    Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
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
                    });
                ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
                on_message.forget();

                let on_open = Closure::<dyn FnMut()>::new(move || connected.set(true));
                ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
                on_open.forget();

                let on_close = Closure::<dyn FnMut()>::new(move || connected.set(false));
                ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
                on_close.forget();

                *held
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ws);
            }
            Err(e) => leptos::logging::error!("could not open a shell: {e:?}"),
        }
    });

    let sending = Arc::clone(&socket);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let text = command.get();
        if text.is_empty() {
            return;
        }
        if let Some(ws) = sending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            && ws.send_with_str(&text).is_ok()
        {
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
