//! Repositories and the credentials that reach them.
//!
//! One screen, because they are only ever configured together: a repository
//! is registered in order to add a credential to it, and a credential exists
//! only to be used by a repository.

use leptos::prelude::*;
use shared::source::{Credential, NewCredential, NewRepo, Repo};

use crate::api;
use crate::screen::Screen;

#[component]
pub fn Sources() -> impl IntoView {
    let screen = Screen::new();
    let repos = RwSignal::new(Vec::<Repo>::new());
    let credentials = RwSignal::new(Vec::<Credential>::new());
    let error = RwSignal::new(None::<String>);
    let loaded = RwSignal::new(false);

    let refresh = move || {
        screen.load(async move {
            match api::repos().await {
                Ok(list) => repos.set(list),
                Err(e) => error.set(Some(e.message)),
            }
            match api::credentials().await {
                Ok(list) => credentials.set(list),
                Err(e) => error.set(Some(e.message)),
            }
            loaded.set(true);
        });
    };
    Effect::new(move |_| refresh());

    view! {
        <header class="topbar">
            <h1 class="wordmark">"Sources"</h1>
            <a class="topbar-link" href="/settings">"Back"</a>
        </header>

        <Show when=move || error.get().is_some()>
            <p class="notice" role="alert">{move || error.get().unwrap_or_default()}</p>
        </Show>

        <h2 class="group-heading">"Repositories"</h2>
        {move || {
            let list = repos.get();
            if list.is_empty() {
                view! {
                    <div class="state-note">
                        <p>"No repositories yet."</p>
                        <p>"Add one to deploy stacks from a compose file you keep in Git."</p>
                    </div>
                }
                .into_any()
            } else {
                view! { <RepoRows repos=list /> }.into_any()
            }
        }}
        <NewRepoForm credentials on_added=refresh />

        <h2 class="group-heading">"Credentials"</h2>
        {move || {
            let list = credentials.get();
            if list.is_empty() {
                view! {
                    <div class="state-note">
                        <p>"No credentials yet."</p>
                        <p>"A public repository needs none; a private one needs a token."</p>
                    </div>
                }
                .into_any()
            } else {
                view! { <CredentialRows credentials=list on_change=refresh error /> }.into_any()
            }
        }}
        <NewCredentialForm on_added=refresh />
    }
}

#[component]
fn RepoRows(repos: Vec<Repo>) -> impl IntoView {
    view! {
        <ul class="rows">
            {repos
                .into_iter()
                .map(|repo| {
                    let detail = repo
                        .credential_name
                        .clone()
                        .map_or_else(|| "No credential".to_owned(), |name| format!("Using {name}"));
                    view! {
                        <li class="row">
                            <a class="row-link" href=format!("/repos/{}/discover", repo.id)>
                                <span class="row-bar" data-state="running"></span>
                                <span class="row-name">{repo.url.clone()}</span>
                                <span class="row-detail">{detail}</span>
                                <span class="row-count">"Find stacks"</span>
                            </a>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
}

#[component]
fn CredentialRows(
    credentials: Vec<Credential>,
    on_change: impl Fn() + Copy + Send + Sync + 'static,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let screen = Screen::new();
    view! {
        <ul class="rows">
            {credentials
                .into_iter()
                .map(|credential| {
                    let id = credential.id;
                    let remove = move |_| {
                        screen.act(
                            async move { api::delete_credential(id).await },
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
                                <span class="row-name">{credential.name.clone()}</span>
                                <span class="row-detail">{credential.username.clone()}</span>
                                <button class="row-action" type="button" on:click=remove>
                                    "Remove"
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
fn NewRepoForm(
    credentials: RwSignal<Vec<Credential>>,
    on_added: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let screen = Screen::new();
    let url = RwSignal::new(String::new());
    let credential_id = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get() {
            return;
        }
        busy.set(true);
        error.set(None);
        let new = NewRepo {
            url: url.get(),
            credential_id: credential_id.get().parse::<i64>().ok(),
        };
        screen.act(async move { api::create_repo(&new).await }, move |result| {
            match result {
                Ok(_) => {
                    url.set(String::new());
                    credential_id.set(String::new());
                    on_added();
                }
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };

    view! {
        <form on:submit=submit>
            <Show when=move || error.get().is_some()>
                <p class="notice" role="alert">{move || error.get().unwrap_or_default()}</p>
            </Show>
            <label class="field">
                <span class="field-label">"Repository URL"</span>
                <input
                    class="field-input"
                    type="text"
                    autocapitalize="none"
                    spellcheck="false"
                    placeholder="https://github.com/you/stacks.git"
                    required
                    prop:value=move || url.get()
                    on:input=move |ev| url.set(event_target_value(&ev))
                />
            </label>
            <label class="field">
                <span class="field-label">"Credential"</span>
                <select
                    class="field-input"
                    prop:value=move || credential_id.get()
                    on:change=move |ev| credential_id.set(event_target_value(&ev))
                >
                    <option value="">"None (public repository)"</option>
                    {move || {
                        credentials
                            .get()
                            .into_iter()
                            .map(|c| view! { <option value=c.id.to_string()>{c.name}</option> })
                            .collect_view()
                    }}
                </select>
            </label>
            <button class="button button-quiet" type="submit" disabled=move || busy.get()>
                {move || if busy.get() { "Adding" } else { "Add repository" }}
            </button>
        </form>
    }
}

#[component]
fn NewCredentialForm(on_added: impl Fn() + Copy + Send + Sync + 'static) -> impl IntoView {
    let screen = Screen::new();
    let name = RwSignal::new(String::new());
    let username = RwSignal::new(String::new());
    let secret = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get() {
            return;
        }
        busy.set(true);
        error.set(None);
        let new = NewCredential {
            name: name.get(),
            username: username.get(),
            secret: secret.get(),
        };
        screen.act(
            async move { api::create_credential(&new).await },
            move |result| {
                match result {
                    Ok(_) => {
                        name.set(String::new());
                        username.set(String::new());
                        // Cleared whatever happens next: it is not coming back
                        // from the server, so leaving it on screen serves nobody.
                        secret.set(String::new());
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
            <Show when=move || error.get().is_some()>
                <p class="notice" role="alert">{move || error.get().unwrap_or_default()}</p>
            </Show>
            <label class="field">
                <span class="field-label">"Name"</span>
                <input
                    class="field-input"
                    type="text"
                    placeholder="github"
                    required
                    prop:value=move || name.get()
                    on:input=move |ev| name.set(event_target_value(&ev))
                />
            </label>
            <label class="field">
                <span class="field-label">"Username"</span>
                <input
                    class="field-input"
                    type="text"
                    autocapitalize="none"
                    spellcheck="false"
                    placeholder="x-access-token"
                    prop:value=move || username.get()
                    on:input=move |ev| username.set(event_target_value(&ev))
                />
            </label>
            <label class="field">
                <span class="field-label">"Token"</span>
                <input
                    class="field-input"
                    type="password"
                    autocomplete="off"
                    required
                    prop:value=move || secret.get()
                    on:input=move |ev| secret.set(event_target_value(&ev))
                />
            </label>
            <button class="button button-quiet" type="submit" disabled=move || busy.get()>
                {move || if busy.get() { "Adding" } else { "Add credential" }}
            </button>
            <p class="entry-note">
                "Stored encrypted. GhostDock will not show it again, so keep your own copy."
            </p>
        </form>
    }
}
