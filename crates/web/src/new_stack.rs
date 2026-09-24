//! Registering a stack from a compose file.

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use shared::deployment::NewStack;

use crate::api;
use crate::screen::Screen;

const PLACEHOLDER: &str = "services:
  web:
    image: nginx:alpine
    ports:
      - \"8080:80\"";

/// Creating a stack and editing one use the same form.
///
/// The fields, validation and failure handling are identical; only the words
/// and the call at the end differ, so splitting them would duplicate the
/// interesting part to avoid duplicating the dull part.
#[component]
pub fn NewStackForm(#[prop(optional)] editing: bool) -> impl IntoView {
    let navigate = use_navigate();
    let params = use_params_map();
    let existing = Memo::new(move |_| {
        editing
            .then(|| params.get().get("id").and_then(|v| v.parse::<i64>().ok()))
            .flatten()
    });

    let name = RwSignal::new(String::new());
    let yaml = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let screen = Screen::new();
    let busy = RwSignal::new(false);

    // Editing starts from what is stored, not from an empty box.
    Effect::new(move |_| {
        let Some(id) = existing.get() else { return };
        screen.load(async move {
            match api::stack_with_yaml(id).await {
                Ok((stack, compose)) => {
                    name.set(stack.name);
                    yaml.set(compose);
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    });

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get() {
            return;
        }
        busy.set(true);
        error.set(None);

        let new = NewStack {
            name: name.get(),
            compose_yaml: yaml.get(),
        };
        let navigate = navigate.clone();
        let id = existing.get_untracked();
        screen.act(
            async move {
                match id {
                    Some(id) => api::update_stack(id, &new).await,
                    None => api::create_stack(1, &new).await,
                }
            },
            move |result| match result {
                // Straight to the stack, where the only useful next action is.
                Ok(stack) => navigate(&format!("/stacks/{}", stack.id), Default::default()),
                Err(e) => {
                    error.set(Some(e.message));
                    busy.set(false);
                }
            },
        );
    };

    view! {
        <header class="topbar">
            <h1 class="wordmark">{if editing { "Edit stack" } else { "New stack" }}</h1>
            <a class="topbar-link" href="/">"Cancel"</a>
        </header>

        <form on:submit=submit>
            <Show when=move || error.get().is_some()>
                <p class="notice" role="alert">{move || error.get().unwrap_or_default()}</p>
            </Show>

            <label class="field">
                <span class="field-label">"Name"</span>
                <input
                    class="field-input"
                    type="text"
                    required
                    prop:value=move || name.get()
                    on:input=move |ev| name.set(event_target_value(&ev))
                />
            </label>

            <label class="field">
                <span class="field-label">"Compose file"</span>
                <textarea
                    class="field-input field-code"
                    rows="14"
                    spellcheck="false"
                    autocapitalize="none"
                    placeholder=PLACEHOLDER
                    required
                    prop:value=move || yaml.get()
                    on:input=move |ev| yaml.set(event_target_value(&ev))
                ></textarea>
            </label>

            <button class="button" type="submit" disabled=move || busy.get()>
                {move || {
                    if busy.get() {
                        "Saving"
                    } else if editing {
                        "Save changes"
                    } else {
                        "Save stack"
                    }
                }}
            </button>
            <Show when=move || !editing>
                <p class="entry-note">
                    "Or " <a href="/stacks/new/git">"deploy from a Git repository"</a>
                    " instead."
                </p>
            </Show>
            <p class="entry-note">
                {if editing {
                    "Saving stores the change. Deploy to apply it."
                } else {
                    "Saving registers the stack. Nothing is deployed until you say so."
                }}
            </p>
        </form>
    }
}
