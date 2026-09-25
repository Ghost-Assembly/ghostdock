//! Who can sign in, and your own password.

use leptos::prelude::*;
use shared::auth::{Account, Credentials, PasswordChange};

use crate::api;
use crate::confirm::Confirm;
use crate::load::Load;
use crate::screen::Screen;
use crate::ui::{ErrorNotice, Field, Row, Topbar};

#[component]
pub fn Accounts() -> impl IntoView {
    let screen = Screen::new();
    let accounts = RwSignal::new(Load::<Vec<Account>>::Loading);
    let error = RwSignal::new(None::<String>);

    let refresh = move || {
        screen.load(async move {
            accounts.set(Load::from(api::accounts().await));
        });
    };
    Effect::new(move |_| refresh());

    view! {
        <Topbar title="Accounts" back="/settings" />

        <ErrorNotice error />

        <h2 class="group-heading">"Who can sign in"</h2>
        {move || match accounts.get() {
            Load::Loading => view! { <p class="state-note">"Loading"</p> }.into_any(),
            Load::Failed(message) => view! {
                <div class="state-note">
                    <p>"Could not read the accounts."</p>
                    <p>{message}</p>
                </div>
            }
            .into_any(),
            Load::Ready(list) => {
                view! { <AccountRows accounts=list on_change=refresh error /> }.into_any()
            }
        }}
        <p class="entry-note">
            "Every account can do everything, including managing the others. \
             You cannot remove your own."
        </p>

        <h2 class="group-heading">"Add an account"</h2>
        <NewAccountForm on_added=refresh />

        <h2 class="group-heading">"Your password"</h2>
        <PasswordForm />
    }
}

#[component]
fn AccountRows(
    accounts: Vec<Account>,
    on_change: impl Fn() + Copy + Send + Sync + 'static,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let screen = Screen::new();
    // One removal at a time, so a second pair of taps cannot send it twice.
    let removing = RwSignal::new(false);
    view! {
        <ul class="rows">
            {accounts
                .into_iter()
                .map(|account| {
                    let id = account.id;
                    let detail = if account.you {
                        "You".to_owned()
                    } else {
                        format!("Added {}", crate::time::local(account.created_at, "%d %b %Y"))
                    };
                    let remove = Callback::new(move |()| {
                        if removing.get_untracked() {
                            return;
                        }
                        removing.set(true);
                        error.set(None);
                        screen.act(
                            async move { api::remove_account(id).await },
                            move |result| {
                                match result {
                                    Ok(()) => on_change(),
                                    Err(e) => error.set(Some(e.message)),
                                }
                                removing.set(false);
                            },
                        );
                    });
                    let you = account.you;
                    let username = account.username.clone();
                    view! {
                        <Row state="none" name=account.username detail>
                            <Show when=move || !you>
                                <Confirm
                                    label="Remove"
                                    confirm="Remove account"
                                    subject=username.clone()
                                    row=true
                                    disabled=Signal::derive(move || removing.get())
                                    on_confirm=remove
                                />
                            </Show>
                        </Row>
                    }
                })
                .collect_view()}
        </ul>
    }
}

#[component]
fn NewAccountForm(on_added: impl Fn() + Copy + Send + Sync + 'static) -> impl IntoView {
    let screen = Screen::new();
    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

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
            async move { api::add_account(&credentials).await },
            move |result| {
                match result {
                    Ok(_) => {
                        username.set(String::new());
                        password.set(String::new());
                        on_added();
                    }
                    Err(e) => error.set(Some(e.message)),
                }
                busy.set(false);
            },
        );
    };

    view! {
        <form on:submit=submit>
            <ErrorNotice error />
            <Field label="Username">
                <input
                    class="field-input"
                    type="text"
                    autocapitalize="none"
                    spellcheck="false"
                    autocomplete="off"
                    required
                    prop:value=move || username.get()
                    on:input=move |ev| username.set(event_target_value(&ev))
                />
            </Field>
            <Field label="Password">
                <input
                    class="field-input"
                    type="password"
                    autocomplete="new-password"
                    minlength="12"
                    required
                    prop:value=move || password.get()
                    on:input=move |ev| password.set(event_target_value(&ev))
                />
            </Field>
            <button class="button button-quiet" type="submit" disabled=move || busy.get()>
                {move || if busy.get() { "Adding" } else { "Add account" }}
            </button>
            <p class="entry-note">"At least 12 characters. Tell them the password yourself."</p>
        </form>
    }
}

#[component]
fn PasswordForm() -> impl IntoView {
    let screen = Screen::new();
    let current = RwSignal::new(String::new());
    let new = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let done = RwSignal::new(false);
    let busy = RwSignal::new(false);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get() {
            return;
        }
        busy.set(true);
        error.set(None);
        done.set(false);
        let change = PasswordChange {
            current: current.get(),
            new: new.get(),
        };
        screen.act(
            async move { api::change_password(&change).await },
            move |result| {
                match result {
                    Ok(()) => {
                        current.set(String::new());
                        new.set(String::new());
                        done.set(true);
                    }
                    Err(e) => error.set(Some(e.message)),
                }
                busy.set(false);
            },
        );
    };

    view! {
        <form on:submit=submit>
            <ErrorNotice error />
            <Field label="Current password">
                <input
                    class="field-input"
                    type="password"
                    autocomplete="current-password"
                    required
                    prop:value=move || current.get()
                    on:input=move |ev| current.set(event_target_value(&ev))
                />
            </Field>
            <Field label="New password">
                <input
                    class="field-input"
                    type="password"
                    autocomplete="new-password"
                    minlength="12"
                    required
                    prop:value=move || new.get()
                    on:input=move |ev| new.set(event_target_value(&ev))
                />
            </Field>
            <button class="button button-quiet" type="submit" disabled=move || busy.get()>
                {move || if busy.get() { "Changing" } else { "Change password" }}
            </button>
            <p class="entry-note">
                {move || if done.get() {
                    "Changed. Every other device signed in as you has been signed out."
                } else {
                    "Changing it signs you out everywhere else."
                }}
            </p>
        </form>
    }
}
