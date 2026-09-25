//! A repository's page: finding the stacks in it and registering several
//! at once, and removing it.

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use shared::source::{DiscoverRequest, DiscoveredStatus, Discovery, ImportRequest, ImportResult};

use crate::api;
use crate::confirm::Confirm;
use crate::screen::Screen;

#[component]
pub fn DiscoverStacks() -> impl IntoView {
    let screen = Screen::new();
    let params = use_params_map();
    let repo_id = Memo::new(move |_| {
        params
            .get()
            .get("id")
            .and_then(|id| id.parse::<i64>().ok())
            .unwrap_or_default()
    });
    let repo_url = RwSignal::new(String::new());
    let credentials = RwSignal::new(Vec::<shared::source::Credential>::new());
    let credential = RwSignal::new(None::<i64>);
    let saved = RwSignal::new(None::<String>);
    let git_ref = RwSignal::new("refs/heads/main".to_owned());
    let pattern = RwSignal::new(String::new());
    let discovery = RwSignal::new(None::<Discovery>);
    let chosen = RwSignal::new(Vec::<String>::new());
    let imported = RwSignal::new(None::<ImportResult>);
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let removing = RwSignal::new(false);
    // The branch and pattern the list on screen came from. Registering uses
    // these, not whatever the fields say now: someone who edits a field
    // after looking must not register paths from one branch against another.
    let looked = StoredValue::new(None::<(String, String)>);

    Effect::new(move |_| {
        let id = repo_id.get();
        screen.load(async move {
            if let Ok(repos) = api::repos().await
                && let Some(repo) = repos.into_iter().find(|r| r.id == id)
            {
                repo_url.set(repo.url);
                credential.set(repo.credential_id);
            }
            if let Ok(list) = api::credentials().await {
                credentials.set(list);
            }
        });
    });

    let navigate = use_navigate();
    let remove = Callback::new(move |()| {
        if removing.get_untracked() {
            return;
        }
        removing.set(true);
        error.set(None);
        let navigate = navigate.clone();
        screen.act(
            async move { api::delete_repo(repo_id.get_untracked()).await },
            move |result| match result {
                Ok(()) => navigate("/sources", Default::default()),
                Err(e) => {
                    error.set(Some(e.message));
                    removing.set(false);
                }
            },
        );
    });

    let save_credential = move |_| {
        error.set(None);
        saved.set(None);
        let chosen = credential.get_untracked();
        screen.act(
            async move { api::set_repo_credential(repo_id.get_untracked(), chosen).await },
            move |result| match result {
                Ok(repo) => saved.set(Some(repo.credential_name.map_or_else(
                    || "Saved. No credential will be sent.".to_owned(),
                    |name| format!("Saved. {name} will be used from now on."),
                ))),
                Err(e) => error.set(Some(e.message)),
            },
        );
    };

    let look = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        imported.set(None);
        let request = DiscoverRequest {
            git_ref: git_ref.get_untracked(),
            pattern: Some(pattern.get_untracked()).filter(|p| !p.trim().is_empty()),
        };
        let asked_ref = request.git_ref.clone();
        screen.load(async move {
            match api::discover(repo_id.get_untracked(), &request).await {
                Ok(found) => {
                    // Everything registrable is chosen to begin with; the
                    // list is for unticking the few that are not wanted.
                    chosen.set(
                        found
                            .found
                            .iter()
                            .filter(|f| f.status == DiscoveredStatus::New)
                            .map(|f| f.path.clone())
                            .collect(),
                    );
                    pattern.set(found.pattern.clone());
                    looked.set_value(Some((asked_ref, found.pattern.clone())));
                    discovery.set(Some(found));
                }
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };

    let register = move |_| {
        if busy.get_untracked() {
            return;
        }
        let Some((git_ref, pattern)) = looked.get_value() else {
            return;
        };
        busy.set(true);
        error.set(None);
        let request = ImportRequest {
            git_ref,
            pattern: Some(pattern),
            paths: chosen.get_untracked(),
        };
        screen.act(
            async move { api::import(repo_id.get_untracked(), &request).await },
            move |result| {
                match result {
                    Ok(done) => {
                        imported.set(Some(done));
                        discovery.set(None);
                        looked.set_value(None);
                        chosen.set(Vec::new());
                    }
                    Err(e) => error.set(Some(e.message)),
                }
                busy.set(false);
            },
        );
    };

    view! {
        <header class="topbar">
            <h1 class="wordmark">"Repository"</h1>
            <a class="topbar-link" href="/sources">"Back"</a>
        </header>

        <p class="entry-note field-mono">{move || repo_url.get()}</p>

        <Show when=move || error.get().is_some()>
            <p class="notice" role="alert">{move || error.get().unwrap_or_default()}</p>
        </Show>

        <form on:submit=look>
            <label class="field">
                <span class="field-label">"Branch or tag"</span>
                <input
                    class="field-input field-mono"
                    type="text"
                    autocapitalize="none"
                    spellcheck="false"
                    required
                    prop:value=move || git_ref.get()
                    on:input=move |ev| git_ref.set(event_target_value(&ev))
                />
            </label>
            <label class="field">
                <span class="field-label">"Look for"</span>
                <input
                    class="field-input field-mono"
                    type="text"
                    autocapitalize="none"
                    spellcheck="false"
                    placeholder="compose.yaml in any folder, or compose/*.yml"
                    prop:value=move || pattern.get()
                    on:input=move |ev| pattern.set(event_target_value(&ev))
                />
            </label>
            <button class="button button-quiet" type="submit" disabled=move || busy.get()>
                {move || if busy.get() { "Looking" } else { "Look" }}
            </button>
            <p class="entry-note">
                "* matches within a folder, ** across folders, {a,b} either. \
                 Separate several patterns with commas."
            </p>
        </form>

        {move || discovery.get().map(|found| {
            let count = found.found.len();
            view! {
                <h2 class="group-heading">
                    {format!(
                        "{count} compose {} at {}",
                        if count == 1 { "file" } else { "files" },
                        shared::short(&found.commit, 12),
                    )}
                </h2>
                <fieldset class="checks">
                    {found.found.into_iter().map(|f| {
                        let path = f.path.clone();
                        let open = f.status == DiscoveredStatus::New;
                        let detail = match &f.status {
                            DiscoveredStatus::New => f.path.clone(),
                            DiscoveredStatus::Registered { stack_name, .. } => {
                                format!("{}: already registered as {stack_name}", f.path)
                            }
                            DiscoveredStatus::NameTaken { stack_name } => {
                                format!("{}: the name is taken by {stack_name}", f.path)
                            }
                        };
                        let toggle_path = path.clone();
                        view! {
                            <label class="check">
                                <input
                                    type="checkbox"
                                    disabled=!open
                                    prop:checked=move || chosen.get().contains(&path)
                                    on:change=move |_| {
                                        let p = toggle_path.clone();
                                        chosen.update(|list| {
                                            if let Some(i) = list.iter().position(|x| *x == p) {
                                                list.remove(i);
                                            } else {
                                                list.push(p);
                                            }
                                        });
                                    }
                                />
                                <span class="check-name">{f.name.clone()}</span>
                                <span class="check-detail">{detail}</span>
                            </label>
                        }
                    }).collect_view()}
                </fieldset>
                <button
                    class="button"
                    type="button"
                    disabled=move || busy.get() || chosen.get().is_empty()
                    on:click=register
                >
                    {move || match chosen.get().len() {
                        0 => "Nothing chosen".to_owned(),
                        1 => "Register 1 stack".to_owned(),
                        n => format!("Register {n} stacks"),
                    }}
                </button>
                <p class="entry-note">
                    "Registering deploys nothing. Each stack waits for you to deploy it, \
                     or to turn on auto-apply."
                </p>
            }
        })}

        {move || imported.get().map(|done| view! {
            <h2 class="group-heading">
                {format!("Registered {}", done.created.len())}
            </h2>
            <ul class="rows">
                {done.created.into_iter().map(|s| view! {
                    <li class="row">
                        <a class="row-link" href=format!("/stacks/{}", s.id)>
                            <span class="row-bar" data-state="stopped"></span>
                            <span class="row-name">{s.name.clone()}</span>
                            <span class="row-detail">
                                {s.git.map(|g| g.compose_path).unwrap_or_default()}
                            </span>
                        </a>
                    </li>
                }).collect_view()}
            </ul>
            <Show when={
                let skipped = done.skipped.len();
                move || skipped > 0
            }>
                <p class="entry-note">
                    {done
                        .skipped
                        .iter()
                        .map(|(path, why)| format!("{path}: {why}."))
                        .collect::<Vec<_>>()
                        .join(" ")}
                </p>
            </Show>
        })}

        <h2 class="group-heading">"Credential"</h2>
        <label class="field">
            <span class="field-label">"Reach it with"</span>
            <select
                class="field-input"
                on:change=move |ev| credential.set(event_target_value(&ev).parse().ok())
            >
                <option value="" selected=move || credential.get().is_none()>
                    "None (public repository)"
                </option>
                {move || credentials.get().into_iter().map(|c| {
                    let id = c.id;
                    view! {
                        <option value=id.to_string() selected=move || credential.get() == Some(id)>
                            {c.name}
                        </option>
                    }
                }).collect_view()}
            </select>
        </label>
        <button class="button button-quiet" type="button" on:click=save_credential>
            "Save credential"
        </button>
        <p class="entry-note">
            {move || saved.get().unwrap_or_else(|| {
                "Tokens expire. Add the new one under Sources, choose it here, and every \
                 stack from this repository uses it."
                    .to_owned()
            })}
        </p>

        <h2 class="group-heading">"Danger"</h2>
        <Confirm
            label="Remove repository"
            confirm="Remove it"
            disabled=Signal::derive(move || removing.get())
            on_confirm=remove
        />
        <p class="entry-note">
            "Only possible once no stack is registered from it. Nothing it deployed is touched."
        </p>
    }
}
