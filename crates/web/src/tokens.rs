//! API tokens: what an outside client, such as an AI assistant, may do.

use leptos::prelude::*;
use shared::token::{ApiToken, NewApiToken, Permission};

use crate::api;
use crate::screen::Screen;

#[component]
pub fn Tokens() -> impl IntoView {
    let screen = Screen::new();
    let tokens = RwSignal::new(Vec::<ApiToken>::new());
    let error = RwSignal::new(None::<String>);
    let revealed = RwSignal::new(None::<(String, String)>);

    let refresh = move || {
        screen.load(async move {
            match api::tokens().await {
                Ok(list) => tokens.set(list),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    Effect::new(move |_| refresh());

    view! {
        <header class="topbar">
            <h1 class="wordmark">"API tokens"</h1>
            <a class="topbar-link" href="/settings">"Back"</a>
        </header>

        <Show when=move || error.get().is_some()>
            <p class="notice" role="alert">{move || error.get().unwrap_or_default()}</p>
        </Show>

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
            <pre class="secret">{secret}</pre>
            <p class="entry-note">
                "Copy it now. GhostDock keeps only a fingerprint and cannot show it again. \
                 Send it as the header Authorization: Bearer followed by the token."
            </p>
            <h2 class="group-heading">"Use it from Claude Code"</h2>
            <pre class="secret">{claude}</pre>
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
        {move || {
            let list = tokens.get();
            if list.is_empty() {
                view! {
                    <div class="state-note">
                        <p>"No tokens yet."</p>
                        <p>"A token lets another program use GhostDock as you, within limits you set."</p>
                    </div>
                }
                .into_any()
            } else {
                view! { <TokenRows tokens=list on_change=refresh error /> }.into_any()
            }
        }}

        <h2 class="group-heading">"New token"</h2>
        <NewTokenForm on_created=move |name, secret| {
            revealed.set(Some((name, secret)));
            refresh();
        } />
    }
}

#[component]
fn TokenRows(
    tokens: Vec<ApiToken>,
    on_change: impl Fn() + Copy + Send + Sync + 'static,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let screen = Screen::new();
    view! {
        <ul class="rows">
            {tokens
                .into_iter()
                .map(|token| {
                    let id = token.id;
                    let armed = RwSignal::new(false);
                    let granted = match token.permissions.as_slice() {
                        [only] => only.as_str().to_owned(),
                        all => format!("{} permissions", all.len()),
                    };
                    let used = token.last_used_at.map_or_else(
                        || "never used".to_owned(),
                        |t| format!("used {}", t.format("%d %b %H:%M")),
                    );
                    let expiry = token.expires_at.map_or_else(
                        || "no expiry".to_owned(),
                        |t| format!("expires {}", t.format("%d %b %Y")),
                    );
                    let revoke = move |_| {
                        if !armed.get_untracked() {
                            armed.set(true);
                            return;
                        }
                        screen.act(
                            async move { api::revoke_token(id).await },
                            move |result| {
                                match result {
                                    Ok(()) => on_change(),
                                    Err(e) => error.set(Some(e.message)),
                                }
                            },
                        );
                    };
                    view! {
                        <li class="row">
                            <span class="row-link">
                                <span class="row-bar" data-state="running"></span>
                                <span class="row-name">{format!("{} ({}…)", token.name, token.prefix)}</span>
                                <span class="row-detail">{format!("{granted}; {used}; {expiry}")}</span>
                                <button class="row-action" type="button" on:click=revoke>
                                    {move || if armed.get() { "Confirm" } else { "Revoke" }}
                                </button>
                            </span>
                        </li>
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
            <Show when=move || error.get().is_some()>
                <p class="notice" role="alert">{move || error.get().unwrap_or_default()}</p>
            </Show>
            <label class="field">
                <span class="field-label">"Name"</span>
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
            </label>
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
                                        prop:checked=move || chosen.get().contains(&p)
                                        on:change=move |_| chosen.update(|list| {
                                            if let Some(i) = list.iter().position(|x| *x == p) {
                                                list.remove(i);
                                            } else {
                                                list.push(p);
                                            }
                                        })
                                    />
                                    <span class="check-name">{p.as_str()}</span>
                                    <span class="check-detail">{p.describe()}</span>
                                </label>
                            })
                            .collect_view()}
                    </fieldset>
                })
                .collect_view()}
            <label class="field">
                <span class="field-label">"Expires"</span>
                <select
                    class="field-input"
                    on:change=move |ev| expiry.set(event_target_value(&ev))
                >
                    <option value="30">"In 30 days"</option>
                    <option value="90" selected>"In 90 days"</option>
                    <option value="365">"In a year"</option>
                    <option value="never">"Never"</option>
                </select>
            </label>
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
