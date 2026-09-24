//! A stack's environment variables.
//!
//! Values are never shown, because the server never returns them. That is
//! deliberate, so the screen is built around it: existing variables are
//! listed by name and can be replaced or removed one at a time, rather than
//! presenting a form that would silently lose every secret not retyped.

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use shared::source::EnvValue;

use crate::api;
use crate::screen::Screen;

#[component]
pub fn StackEnvironment() -> impl IntoView {
    let params = use_params_map();
    let id = Memo::new(move |_| {
        params
            .get()
            .get("id")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or_default()
    });

    let keys = RwSignal::new(Vec::<String>::new());
    let error = RwSignal::new(None::<String>);
    let screen = Screen::new();
    let key = RwSignal::new(String::new());
    let value = RwSignal::new(String::new());
    let busy = RwSignal::new(false);

    let refresh = move || {
        let stack = id.get_untracked();
        screen.load(async move {
            match api::stack_env_keys(stack).await {
                Ok(listed) => keys.set(listed.keys),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    Effect::new(move |_| {
        let _ = id.get();
        refresh();
    });

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get() {
            return;
        }
        busy.set(true);
        error.set(None);
        let name = key.get();
        let body = EnvValue { value: value.get() };
        let stack = id.get_untracked();
        screen.act(
            async move { api::set_stack_env_one(stack, &name, &body).await },
            move |result| {
                match result {
                    Ok(listed) => {
                        keys.set(listed.keys);
                        key.set(String::new());
                        value.set(String::new());
                    }
                    Err(e) => error.set(Some(e.message)),
                }
                busy.set(false);
            },
        );
    };

    view! {
        <header class="topbar">
            <h1 class="wordmark">"Environment"</h1>
            <a class="topbar-link" href=move || format!("/stacks/{}", id.get())>"Back"</a>
        </header>

        <Show when=move || error.get().is_some()>
            <p class="notice" role="alert">{move || error.get().unwrap_or_default()}</p>
        </Show>

        {move || {
            let list = keys.get();
            if list.is_empty() {
                view! {
                    <div class="state-note">
                        <p>"No variables yet."</p>
                        <p>"Anything you add here is encrypted and written to the stack's .env."</p>
                    </div>
                }
                .into_any()
            } else {
                view! {
                    <ul class="rows">
                        {list
                            .into_iter()
                            .map(|name| {
                                let removing = name.clone();
                                let remove = move |_| {
                                    let removing = removing.clone();
                                    let stack = id.get_untracked();
                                    screen.act(
                                        async move {
                                            api::delete_stack_env_one(stack, &removing).await
                                        },
                                        move |result| match result {
                                            Ok(listed) => keys.set(listed.keys),
                                            Err(e) => error.set(Some(e.message)),
                                        },
                                    );
                                };
                                view! {
                                    <li class="row">
                                        <span class="row-link">
                                            <span class="row-bar" data-state="running"></span>
                                            <span class="row-name">{name.clone()}</span>
                                            <span class="row-detail">"set"</span>
                                            <button
                                                class="row-action"
                                                type="button"
                                                on:click=remove
                                            >
                                                "Remove"
                                            </button>
                                        </span>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                }
                .into_any()
            }
        }}

        <h2 class="group-heading">"Add or replace"</h2>
        <form on:submit=submit>
            <label class="field">
                <span class="field-label">"Name"</span>
                <input
                    class="field-input"
                    type="text"
                    autocapitalize="characters"
                    spellcheck="false"
                    placeholder="API_KEY"
                    required
                    prop:value=move || key.get()
                    on:input=move |ev| key.set(event_target_value(&ev))
                />
            </label>
            <label class="field">
                <span class="field-label">"Value"</span>
                <input
                    class="field-input"
                    type="password"
                    autocomplete="off"
                    prop:value=move || value.get()
                    on:input=move |ev| value.set(event_target_value(&ev))
                />
            </label>
            <button class="button button-quiet" type="submit" disabled=move || busy.get()>
                {move || if busy.get() { "Saving" } else { "Save variable" }}
            </button>
            <p class="entry-note">
                "Values are encrypted and never shown again. Deploy to apply a change."
            </p>
        </form>
    }
}
