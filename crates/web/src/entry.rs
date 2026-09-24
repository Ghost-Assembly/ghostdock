//! First-run setup and sign-in.
//!
//! One component serves both because the form is identical; only the stakes
//! and the words differ. Splitting them would duplicate the field handling to
//! no benefit.

use leptos::prelude::*;
use shared::auth::Credentials;

use crate::api;
use crate::app::Session;
use crate::screen::Screen;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EntryMode {
    Setup,
    Login,
}

#[component]
pub fn Entry(mode: EntryMode, session: RwSignal<Session>) -> impl IntoView {
    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(Option::<String>::None);
    let busy = RwSignal::new(false);
    let screen = Screen::new();

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get() {
            return;
        }
        busy.set(true);
        error.set(None);

        let credentials = Credentials {
            username: username.get(),
            password: password.get(),
        };

        screen.act(
            async move {
                match mode {
                    EntryMode::Setup => api::bootstrap(&credentials).await,
                    EntryMode::Login => api::login(&credentials).await,
                }
            },
            move |result| match result {
                Ok(user) => session.set(Session::SignedIn(user)),
                Err(e) => {
                    error.set(Some(e.message));
                    password.set(String::new());
                    busy.set(false);
                }
            },
        );
    };

    let heading = match mode {
        EntryMode::Setup => "Create your administrator account",
        EntryMode::Login => "Sign in to GhostDock",
    };

    // Said before they type, not after the server rejects them.
    let note = match mode {
        EntryMode::Setup => {
            "This is the only account until you add another. \
             Use at least 12 characters — GhostDock can control every container \
             on this host."
        }
        EntryMode::Login => "",
    };

    let action = match mode {
        EntryMode::Setup => "Create account",
        EntryMode::Login => "Sign in",
    };

    view! {
        <section class="entry">
            <div>
                <h1 class="entry-heading">{heading}</h1>
                <Show when=move || !note.is_empty()>
                    <p class="entry-note">{note}</p>
                </Show>
            </div>

            <form on:submit=submit>
                <Show when=move || error.get().is_some()>
                    <p class="notice" role="alert">{move || error.get().unwrap_or_default()}</p>
                </Show>

                <label class="field">
                    <span class="field-label">"Username"</span>
                    <input
                        class="field-input"
                        type="text"
                        autocomplete="username"
                        autocapitalize="none"
                        spellcheck="false"
                        required
                        prop:value=move || username.get()
                        on:input=move |ev| username.set(event_target_value(&ev))
                    />
                </label>

                <label class="field">
                    <span class="field-label">"Password"</span>
                    <input
                        class="field-input"
                        type="password"
                        autocomplete=move || match mode {
                            EntryMode::Setup => "new-password",
                            EntryMode::Login => "current-password",
                        }
                        required
                        prop:value=move || password.get()
                        on:input=move |ev| password.set(event_target_value(&ev))
                    />
                </label>

                <button class="button" type="submit" disabled=move || busy.get()>
                    {move || if busy.get() { "Working" } else { action }}
                </button>
            </form>
        </section>
    }
}
