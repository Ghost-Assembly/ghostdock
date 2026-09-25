//! A stack's environment variables.
//!
//! Values are never shown, because the server never returns them. That is
//! deliberate, so the screen is built around it: existing variables are
//! listed by name and can be replaced or removed one at a time, rather than
//! presenting a form that would silently lose every secret not retyped.

use leptos::prelude::*;
use shared::source::EnvValue;

use crate::api;
use crate::confirm::Confirm;
use crate::load::Load;
use crate::screen::Screen;
use crate::ui::{ErrorNotice, Field, Row, Topbar, route_id};

/// A name the server will take: letters, digits and underscores, not
/// starting with a digit. Checked here too, so a typo is explained before
/// anything is sent rather than refused after.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[component]
pub fn StackEnvironment() -> impl IntoView {
    let id = route_id();

    let keys = RwSignal::new(Load::<Vec<String>>::Loading);
    let error = RwSignal::new(None::<String>);
    let screen = Screen::new();
    let key = RwSignal::new(String::new());
    let value = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    // One removal at a time, so a second pair of taps cannot send it twice.
    let removing = RwSignal::new(false);

    let refresh = move || {
        let stack = id.get_untracked();
        screen.load(async move {
            keys.set(Load::from(
                api::stack_env_keys(stack).await.map(|listed| listed.keys),
            ));
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
        let name = key.get();
        if !valid_name(&name) {
            error.set(Some(
                "A name uses letters, digits and underscores, and does not start with a digit."
                    .to_owned(),
            ));
            return;
        }
        busy.set(true);
        error.set(None);
        let body = EnvValue { value: value.get() };
        let stack = id.get_untracked();
        screen.act(
            async move { api::set_stack_env_one(stack, &name, &body).await },
            move |result| {
                match result {
                    Ok(listed) => {
                        keys.set(Load::Ready(listed.keys));
                        key.set(String::new());
                        value.set(String::new());
                    }
                    Err(e) => error.set(Some(e.message)),
                }
                busy.set(false);
            },
        );
    };

    let remove = move |name: String| {
        Callback::new(move |()| {
            if removing.get_untracked() {
                return;
            }
            removing.set(true);
            error.set(None);
            let name = name.clone();
            let stack = id.get_untracked();
            screen.act(
                async move { api::delete_stack_env_one(stack, &name).await },
                move |result| {
                    match result {
                        Ok(listed) => keys.set(Load::Ready(listed.keys)),
                        Err(e) => error.set(Some(e.message)),
                    }
                    removing.set(false);
                },
            );
        })
    };

    view! {
        <Topbar title="Environment" back=Signal::derive(move || format!("/stacks/{}", id.get())) />

        <ErrorNotice error />

        {move || match keys.get() {
            Load::Loading => view! { <p class="state-note">"Loading"</p> }.into_any(),
            Load::Failed(message) => view! {
                <div class="state-note">
                    <p>"Could not read the variables."</p>
                    <p>{message}</p>
                </div>
            }
            .into_any(),
            Load::Ready(list) if list.is_empty() => view! {
                <div class="state-note">
                    <p>"No variables yet."</p>
                    <p>"Anything you add here is encrypted and written to the stack's .env."</p>
                </div>
            }
            .into_any(),
            Load::Ready(list) => view! {
                <ul class="rows">
                    {list
                        .into_iter()
                        .map(|name| {
                            view! {
                                <Row state="none" name=name.clone() ident=true detail="set">
                                    <Confirm
                                        label="Remove"
                                        confirm="Remove it"
                                        subject=name.clone()
                                        row=true
                                        disabled=Signal::derive(move || removing.get())
                                        on_confirm=remove(name)
                                    />
                                </Row>
                            }
                        })
                        .collect_view()}
                </ul>
            }
            .into_any(),
        }}

        <h2 class="group-heading">"Add or replace"</h2>
        <form on:submit=submit>
            <Field label="Name">
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
            </Field>
            <Field label="Value">
                <input
                    class="field-input"
                    type="password"
                    autocomplete="off"
                    prop:value=move || value.get()
                    on:input=move |ev| value.set(event_target_value(&ev))
                />
            </Field>
            <button class="button button-quiet" type="submit" disabled=move || busy.get()>
                {move || if busy.get() { "Saving" } else { "Save variable" }}
            </button>
            <p class="entry-note">
                "Values are encrypted and never shown again. Deploy to apply a change."
            </p>
        </form>
    }
}

#[cfg(test)]
mod tests {
    use super::valid_name;

    #[test]
    fn a_name_is_what_the_server_accepts() {
        for ok in ["API_KEY", "_private", "a1", "X"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "1ST", "BAD-NAME", "A/B", "..", "SPACE D", "É"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }
}
