//! Application shell and session gate.

use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::hooks::{use_location, use_navigate};
use leptos_router::path;
use shared::auth::User;

use crate::accounts::Accounts;
use crate::activity::Activity;
use crate::api;
use crate::cleanup::Cleanup;
use crate::console::Console;
use crate::deployment::DeploymentView;
use crate::discover::DiscoverStacks;
use crate::entry::{Entry, EntryMode};
use crate::env::StackEnvironment;
use crate::events;
use crate::git_stack::NewGitStackForm;
use crate::host::Host;
use crate::logs::ContainerLogs;
use crate::new_stack::NewStackForm;
use crate::resources::ContainerResources;
use crate::screen::Screen;
use crate::settings::Settings;
use crate::sources::Sources;
use crate::stack::StackDetail;
use crate::stacks::Stacks;
use crate::tokens::Tokens;
use crate::updates::Updates;

/// Who the viewer is, as far as the client knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Session {
    Loading,
    /// No administrator exists yet; the only thing to do is create one.
    NeedsSetup,
    SignedOut,
    SignedIn(User),
    /// The API itself could not be reached.
    Unreachable(String),
}

#[component]
pub fn App() -> impl IntoView {
    let session = RwSignal::new(Session::Loading);
    // Any view can retire the session when the server says the cookie is
    // no longer good, so an expiry mid-use shows the sign-in screen rather
    // than an authentication error where the content should be.
    provide_context(session);
    // Whichever screen hears it first. Only a signed-in session is retired:
    // a wrong password on the sign-in screen is a 401 too.
    api::on_unauthenticated(move || {
        if matches!(session.get_untracked(), Session::SignedIn(_)) {
            session.set(Session::SignedOut);
        }
    });

    let screen = Screen::new();
    let check = move || {
        screen.load(async move {
            session.set(match api::auth_status().await {
                Ok(status) => match status.user {
                    Some(user) => Session::SignedIn(user),
                    None if status.bootstrapped => Session::SignedOut,
                    None => Session::NeedsSetup,
                },
                Err(e) => Session::Unreachable(e.message),
            });
        });
    };
    check();

    // An installed app has no reload button. Coming back online is the
    // moment the server may be reachable again, so look then.
    let _ = window_event_listener(leptos::ev::online, move |_| {
        if matches!(session.get_untracked(), Session::Unreachable(_)) {
            check();
        }
    });

    view! {
        {move || match session.get() {
            Session::Loading => view! {
                <p class="state-note">"Loading"</p>
            }
            .into_any(),
            Session::Unreachable(message) => view! {
                <section class="entry">
                    <div>
                        <h1 class="entry-heading">"GhostDock is not responding"</h1>
                        <p class="entry-note">{message}</p>
                        <p class="entry-note">
                            "Check that the server is running and this device is online."
                        </p>
                        <button class="button" type="button" on:click=move |_| {
                            session.set(Session::Loading);
                            check();
                        }>
                            "Try again"
                        </button>
                    </div>
                </section>
            }
            .into_any(),
            Session::NeedsSetup => view! { <Entry mode=EntryMode::Setup session /> }.into_any(),
            Session::SignedOut => view! { <Entry mode=EntryMode::Login session /> }.into_any(),
            Session::SignedIn(user) => view! { <Shell user session /> }.into_any(),
        }}
    }
}

#[component]
fn Shell(user: User, session: RwSignal<Session>) -> impl IntoView {
    // Opened once the viewer is known, so an anonymous page load does not
    // hold a connection the server would only reject.
    let paused = events::provide("/api/v1/events/socket").paused();

    view! {
        <Router>
            <main class="shell">
                // Said, because a screen that stops changing looks exactly
                // like one where nothing is happening.
                <Show when=move || paused.get()>
                    <p class="live-paused" role="status">"Live updates paused. Reconnecting."</p>
                </Show>
                <Routes fallback=|| {
                    view! { <p class="state-note">"That page does not exist."</p> }
                }>
                    <Route path=path!("/") view=Stacks />
                    <Route path=path!("/stacks/new") view=|| view! { <NewStackForm /> } />
                    <Route path=path!("/stacks/:id") view=StackDetail />
                    <Route
                        path=path!("/stacks/:id/edit")
                        view=|| view! { <NewStackForm editing=true /> }
                    />
                    <Route path=path!("/stacks/new/git") view=NewGitStackForm />
                    <Route path=path!("/stacks/:id/env") view=StackEnvironment />
                    <Route path=path!("/sources") view=Sources />
                    <Route path=path!("/repos/:id/discover") view=DiscoverStacks />
                    <Route path=path!("/updates") view=Updates />
                    <Route path=path!("/host") view=Host />
                    <Route path=path!("/cleanup") view=Cleanup />
                    <Route path=path!("/activity") view=Activity />
                    <Route path=path!("/accounts") view=Accounts />
                    <Route path=path!("/tokens") view=Tokens />
                    <Route path=path!("/containers/:id/logs") view=ContainerLogs />
                    <Route path=path!("/containers/:id/shell") view=Console />
                    <Route path=path!("/containers/:name/resources") view=ContainerResources />
                    <Route path=path!("/deployments/:id") view=DeploymentView />
                    <Route
                        path=path!("/settings")
                        view=move || view! { <Settings user=user.clone() session /> }
                    />
                </Routes>
            </main>
            <Nav />
        </Router>
    }
}

/// Bottom navigation.
///
/// Only surfaces destinations that exist. A nav item leading to "coming soon"
/// spends a user's attention on nothing.
#[component]
fn Nav() -> impl IntoView {
    let location = use_location();
    let navigate = use_navigate();

    // A section stays current on every screen within it: a stack's page is
    // still under Stacks, repositories and tokens under Settings.
    let item = move |href: &'static str, label: &'static str, within: &'static [&'static str]| {
        let navigate = navigate.clone();
        let current = Memo::new(move |_| {
            let path = location.pathname.get();
            path == href || within.iter().any(|prefix| path.starts_with(prefix))
        });
        view! {
            <button
                class="nav-item"
                aria-current=move || if current.get() { Some("page") } else { None }
                on:click=move |_| navigate(href, Default::default())
            >
                {label}
            </button>
        }
    };

    view! {
        <nav class="nav" aria-label="Sections">
            // Shown only where the navigation is a sidebar.
            <span class="nav-brand">"GhostDock"</span>
            {item("/", "Stacks", &["/stacks", "/containers", "/deployments"])}
            {item("/updates", "Updates", &[])}
            {item("/host", "Host", &[])}
            {item(
                "/settings",
                "Settings",
                &["/sources", "/repos", "/cleanup", "/activity", "/accounts", "/tokens"],
            )}
        </nav>
    }
}
