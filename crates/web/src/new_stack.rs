//! Registering a stack from a compose file.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use shared::deployment::NewStack;

use crate::api;
use crate::screen::Screen;
use crate::ui::{ErrorNotice, Field, Topbar, route_id};

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
    let id = route_id();
    // The stack being edited, if any; the route gives 0 for no id.
    let existing = Memo::new(move |_| Some(id.get()).filter(|id| editing && *id > 0));

    let name = RwSignal::new(String::new());
    let yaml = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let screen = Screen::new();
    let busy = RwSignal::new(false);
    // Saving an edit before the stored file has arrived would replace it
    // with whatever half of it had been typed.
    let loaded = RwSignal::new(!editing);
    // Set by typing. What someone typed is theirs: a load that lands late
    // fills only the fields they have not touched.
    let typed_name = StoredValue::new(false);
    let typed_yaml = StoredValue::new(false);

    // Editing starts from what is stored, not from an empty box.
    Effect::new(move |_| {
        let Some(id) = existing.get() else { return };
        loaded.set(false);
        screen.load(async move {
            match api::stack_with_yaml(id).await {
                Ok((stack, compose)) => {
                    if !typed_name.get_value() {
                        name.set(stack.name);
                    }
                    if !typed_yaml.get_value() {
                        yaml.set(compose);
                    }
                    loaded.set(true);
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    });

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get() || !loaded.get() {
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
                    None => api::create_stack(&new).await,
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
        <Topbar title=if editing { "Edit stack" } else { "New stack" }>
            // Editing is left for the stack it edits; a new one, for the board.
            <a class="topbar-link" href=move || {
                existing.get().map_or_else(|| "/".to_owned(), |id| format!("/stacks/{id}"))
            }>"Cancel"</a>
        </Topbar>

        <form on:submit=submit>
            <ErrorNotice error />

            <Field label="Name">
                <input
                    class="field-input"
                    type="text"
                    required
                    prop:value=move || name.get()
                    on:input=move |ev| {
                        typed_name.set_value(true);
                        name.set(event_target_value(&ev));
                    }
                />
            </Field>

            <Field label="Compose file">
                <textarea
                    class="field-input field-code"
                    rows="14"
                    spellcheck="false"
                    autocapitalize="none"
                    placeholder=PLACEHOLDER
                    required
                    prop:value=move || yaml.get()
                    on:input=move |ev| {
                        typed_yaml.set_value(true);
                        yaml.set(event_target_value(&ev));
                    }
                ></textarea>
            </Field>

            <button class="button" type="submit" disabled=move || busy.get() || !loaded.get()>
                {move || {
                    if !loaded.get() {
                        "Loading"
                    } else if busy.get() {
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
