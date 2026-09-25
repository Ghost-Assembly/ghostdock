//! API tokens: what an outside client, such as an AI assistant, may do.

use leptos::prelude::*;
use shared::token::{ApiToken, NewApiToken, Permission};

use crate::api;
use crate::confirm::Confirm;
use crate::load::Load;
use crate::screen::Screen;
use crate::ui::{ErrorNotice, Field, Icon, Row, Topbar, toggle};

#[component]
pub fn Tokens() -> impl IntoView {
    let screen = Screen::new();
    let tokens = RwSignal::new(Load::<Vec<ApiToken>>::Loading);
    let error = RwSignal::new(None::<String>);
    let revealed = RwSignal::new(None::<(String, String)>);

    let refresh = move || {
        screen.load(async move {
            tokens.set(Load::from(api::tokens().await));
        });
    };
    Effect::new(move |_| refresh());

    view! {
        <Topbar title="API tokens" back="/settings" />

        <ErrorNotice error />

        {move || revealed.get().map(|(name, secret)| {
            // The command to hand this token to Claude Code, for this very
            // address, so setting it up is a paste.
            let origin = web_sys::window()
                .and_then(|w| w.location().origin().ok())
                .unwrap_or_default();
            let claude = format!(
                "claude mcp add --transport http ghostdock {origin}/mcp --header \"Authorization: Bearer {secret}\""
            );
            view! {
            <h2 class="group-heading">{format!("New token: {name}")}</h2>
            <pre class="secret">{secret.clone()}</pre>
            <CopyButton text=secret what="token" />
            <p class="entry-note">
                "Copy it now. GhostDock keeps only a fingerprint and cannot show it again. \
                 Send it as the header Authorization: Bearer followed by the token."
            </p>
            <h2 class="group-heading">"Use it from Claude Code"</h2>
            <pre class="secret">{claude.clone()}</pre>
            <CopyButton text=claude what="command" />
            <p class="entry-note">
                "GhostDock's MCP server offers Claude the tools this token's permissions allow, \
                 and nothing else."
            </p>
            <button class="button button-quiet" type="button" on:click=move |_| revealed.set(None)>
                "I have copied it"
            </button>
            }
        })}

        <h2 class="group-heading">"Your tokens"</h2>
        {move || match tokens.get() {
            Load::Loading => view! { <p class="state-note">"Loading"</p> }.into_any(),
            Load::Failed(message) => view! {
                <div class="state-note">
                    <p>"Could not read your tokens."</p>
                    <p>{message}</p>
                </div>
            }
            .into_any(),
            Load::Ready(list) if list.is_empty() => view! {
                <div class="state-note">
                    <p>"No tokens yet."</p>
                    <p>"A token lets another program use GhostDock as you, within limits you set."</p>
                </div>
            }
            .into_any(),
            Load::Ready(list) => view! { <TokenRows tokens=list on_change=refresh error /> }.into_any(),
        }}

        <h2 class="group-heading">"New token"</h2>
        <NewTokenForm on_created=move |name, secret| {
            revealed.set(Some((name, secret)));
            refresh();
        } />
    }
}

/// Copies `text` to the clipboard, and says whether it did.
///
/// Browsers offer the clipboard only over HTTPS or on localhost. Elsewhere
/// the button says so; the text above it selects in one tap, so the
/// phone's own copy still works.
#[component]
fn CopyButton(text: String, what: &'static str) -> impl IntoView {
    let screen = Screen::new();
    let said = RwSignal::new(None::<&'static str>);
    let text = StoredValue::new(text);
    let copy = move |_| {
        let Some(window) = web_sys::window() else {
            return;
        };
        if !window.is_secure_context() {
            said.set(Some(
                "This browser copies only over HTTPS. Tap the text to select it, then copy.",
            ));
            return;
        }
        let promise = window.navigator().clipboard().write_text(&text.get_value());
        screen.act(
            wasm_bindgen_futures::JsFuture::from(promise),
            move |result| {
                said.set(Some(if result.is_ok() {
                    "Copied."
                } else {
                    "Could not copy. Tap the text to select it, then copy."
                }));
            },
        );
    };
    view! {
        <button class="button button-quiet" type="button" on:click=copy>
            <Icon name="copy" />
            {format!("Copy {what}")}
        </button>
        <p class="entry-note" role="status">{move || said.get()}</p>
    }
}

#[component]
fn TokenRows(
    tokens: Vec<ApiToken>,
    on_change: impl Fn() + Copy + Send + Sync + 'static,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let screen = Screen::new();
    // One revocation at a time, so a second pair of taps cannot send it twice.
    let revoking = RwSignal::new(false);
    view! {
        <ul class="rows">
            {tokens
                .into_iter()
                .map(|token| {
                    let id = token.id;
                    let granted = match token.permissions.as_slice() {
                        [only] => only.as_str().to_owned(),
                        all => format!("{} permissions", all.len()),
                    };
                    let used = token.last_used_at.map_or_else(
                        || "never used".to_owned(),
                        |t| format!("used {}", crate::time::local(t, "%d %b %H:%M")),
                    );
                    let expiry = token.expires_at.map_or_else(
                        || "no expiry".to_owned(),
                        |t| format!("expires {}", crate::time::local(t, "%d %b %Y")),
                    );
                    let revoke = Callback::new(move |()| {
                        if revoking.get_untracked() {
                            return;
                        }
                        revoking.set(true);
                        error.set(None);
                        screen.act(
                            async move { api::revoke_token(id).await },
                            move |result| {
                                match result {
                                    Ok(()) => on_change(),
                                    Err(e) => error.set(Some(e.message)),
                                }
                                revoking.set(false);
                            },
                        );
                    });
                    view! {
                        <Row
                            state="none"
                            name=format!("{} ({}…)", token.name, token.prefix)
                            detail=format!("{granted}; {used}; {expiry}")
                        >
                            <Confirm
                                label="Revoke"
                                confirm="Revoke it"
                                subject=token.name
                                row=true
                                disabled=Signal::derive(move || revoking.get())
                                on_confirm=revoke
                            />
                        </Row>
                    }
                })
                .collect_view()}
        </ul>
    }
}

#[component]
fn NewTokenForm(
    on_created: impl Fn(String, String) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let screen = Screen::new();
    let name = RwSignal::new(String::new());
    // Nothing is granted until someone ticks it.
    let chosen = RwSignal::new(Vec::<Permission>::new());
    let expiry = RwSignal::new("90".to_owned());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get() {
            return;
        }
        busy.set(true);
        error.set(None);
        let new = NewApiToken {
            name: name.get(),
            permissions: chosen.get(),
            expires_in_days: expiry.get().parse().ok(),
        };
        screen.act(
            async move { api::create_token(&new).await },
            move |result| {
                match result {
                    Ok(created) => {
                        name.set(String::new());
                        chosen.set(Vec::new());
                        on_created(created.token.name, created.secret);
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
            <Field label="Name">
                <input
                    class="field-input"
                    type="text"
                    placeholder="assistant"
                    autocapitalize="none"
                    spellcheck="false"
                    required
                    prop:value=move || name.get()
                    on:input=move |ev| name.set(event_target_value(&ev))
                />
            </Field>
            <p class="field-label">"It may"</p>
            {Permission::AREAS
                .iter()
                .map(|area| view! {
                    <fieldset class="checks">
                        <legend class="checks-area">{*area}</legend>
                        {Permission::ALL
                            .into_iter()
                            .filter(|p| p.area() == *area)
                            .map(|p| view! {
                                <label class="check">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || chosen.with(|list| list.contains(&p))
                                        on:change=move |_| chosen.update(|list| toggle(list, p))
                                    />
                                    <span class="check-name">{p.as_str()}</span>
                                    <span class="check-detail">{p.describe()}</span>
                                </label>
                            })
                            .collect_view()}
                    </fieldset>
                })
                .collect_view()}
            <Field label="Expires">
                <select
                    class="field-input"
                    on:change=move |ev| expiry.set(event_target_value(&ev))
                >
                    <option value="30">"In 30 days"</option>
                    <option value="90" selected>"In 90 days"</option>
                    <option value="365">"In a year"</option>
                    <option value="never">"Never"</option>
                </select>
            </Field>
            <button class="button button-quiet" type="submit" disabled=move || busy.get()>
                {move || if busy.get() { "Creating" } else { "Create token" }}
            </button>
            <p class="entry-note">
                "Only you can create or revoke your tokens, and no token can create another. \
                 Revoking takes effect at once, closing any shell the token has open."
            </p>
        </form>
    }
}
