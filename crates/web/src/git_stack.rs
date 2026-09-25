//! Registering a stack whose compose file lives in a repository.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use shared::source::{NewGitStack, Repo};

use crate::api;
use crate::screen::Screen;
use crate::ui::{ErrorNotice, Field, Topbar};

#[component]
pub fn NewGitStackForm() -> impl IntoView {
    let repos = RwSignal::new(Vec::<Repo>::new());
    let loaded = RwSignal::new(false);
    let load_error = RwSignal::new(None::<String>);
    let screen = Screen::new();

    Effect::new(move |_| {
        screen.load(async move {
            match api::repos().await {
                Ok(list) => repos.set(list),
                Err(e) => load_error.set(Some(e.message)),
            }
            loaded.set(true);
        });
    });

    view! {
        <Topbar title="Stack from Git">
            <a class="topbar-link" href="/">"Cancel"</a>
        </Topbar>

        <ErrorNotice error=load_error />

        {move || {
            if !loaded.get() {
                return view! { <p class="state-note">"Loading"</p> }.into_any();
            }
            let list = repos.get();
            if list.is_empty() {
                // A form with nothing to select from is worse than saying so.
                return view! {
                    <div class="state-note">
                        <p>"No repositories yet."</p>
                        <p>"Add one first, then come back."</p>
                        <p><a class="button" href="/sources">"Add a repository"</a></p>
                    </div>
                }
                .into_any();
            }
            view! { <Fields repos=list /> }.into_any()
        }}
    }
}

/// The form itself, so its submit closure is owned by a component rather
/// than captured by a conditional wrapper that must be callable repeatedly.
#[component]
fn Fields(repos: Vec<Repo>) -> impl IntoView {
    let screen = Screen::new();
    let navigate = use_navigate();
    let first_repo = repos.first().map(|r| r.id.to_string()).unwrap_or_default();
    let name = RwSignal::new(String::new());
    let repo_id = RwSignal::new(first_repo);
    // A full ref, because a short name can resolve to a tag on one poll and a
    // branch on the next.
    let git_ref = RwSignal::new("refs/heads/main".to_owned());
    let compose_path = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get() {
            return;
        }
        let Ok(repo) = repo_id.get().parse::<i64>() else {
            error.set(Some("Choose a repository first.".to_owned()));
            return;
        };
        busy.set(true);
        error.set(None);

        let new = NewGitStack {
            name: name.get(),
            repo_id: repo,
            git_ref: git_ref.get(),
            compose_path: compose_path.get(),
        };
        let navigate = navigate.clone();
        screen.act(
            async move { api::create_git_stack(&new).await },
            move |result| match result {
                Ok(created) => navigate(&format!("/stacks/{}", created.id), Default::default()),
                Err(e) => {
                    error.set(Some(e.message));
                    busy.set(false);
                }
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
                    required
                    prop:value=move || name.get()
                    on:input=move |ev| name.set(event_target_value(&ev))
                />
            </Field>

            <Field label="Repository">
                <select
                    class="field-input"
                    prop:value=move || repo_id.get()
                    on:change=move |ev| repo_id.set(event_target_value(&ev))
                >
                    {repos
                        .into_iter()
                        .map(|r| view! { <option value=r.id.to_string()>{r.url}</option> })
                        .collect_view()}
                </select>
            </Field>

            <Field label="Branch or tag">
                <input
                    class="field-input field-mono"
                    type="text"
                    autocapitalize="none"
                    spellcheck="false"
                    required
                    prop:value=move || git_ref.get()
                    on:input=move |ev| git_ref.set(event_target_value(&ev))
                />
            </Field>

            <Field label="Compose file in the repository">
                <input
                    class="field-input field-mono"
                    type="text"
                    autocapitalize="none"
                    spellcheck="false"
                    placeholder="compose/blog.yml"
                    required
                    prop:value=move || compose_path.get()
                    on:input=move |ev| compose_path.set(event_target_value(&ev))
                />
            </Field>

            <button class="button" type="submit" disabled=move || busy.get()>
                {move || if busy.get() { "Saving" } else { "Save stack" }}
            </button>
            <p class="entry-note">
                "Saving registers the stack. The file is read from the repository each time you deploy."
            </p>
        </form>
    }
}
