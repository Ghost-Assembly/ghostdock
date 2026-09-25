//! Account and host details.

use leptos::prelude::*;
use shared::auth::User;
use shared::host::HostInfo;

use crate::api;
use crate::app::Session;
use crate::load::Load;
use crate::screen::Screen;

#[component]
pub fn Settings(user: User, session: RwSignal<Session>) -> impl IntoView {
    let info = RwSignal::new(Load::<HostInfo>::Loading);
    let screen = Screen::new();
    let signing_out = RwSignal::new(false);
    let sign_out_error = RwSignal::new(None::<String>);

    screen.load(async move {
        info.set(match api::hosts().await {
            Ok(hosts) => match hosts.first() {
                Some(host) => Load::from(api::host_info(host.id).await),
                None => Load::Failed("GhostDock has no host to show.".to_owned()),
            },
            Err(e) => Load::Failed(e.message),
        });
    });

    // Signed out only once the server says so: a sign-in screen shown while
    // the session is still good would be a lie, and a dangerous one on a
    // shared device. The session is the whole app's, not this screen's, so
    // it is updated as part of the request: signing out lands wherever you
    // have gone.
    let sign_out = move |_| {
        if signing_out.get_untracked() {
            return;
        }
        signing_out.set(true);
        sign_out_error.set(None);
        screen.act(
            async move {
                let result = api::logout().await;
                if result.is_ok() {
                    session.set(Session::SignedOut);
                }
                result
            },
            move |result| {
                if let Err(e) = result {
                    sign_out_error.set(Some(format!("Still signed in. {}", e.message)));
                }
                signing_out.set(false);
            },
        );
    };

    view! {
        <header class="topbar">
            <h1 class="wordmark">"Settings"</h1>
        </header>

        <h2 class="group-heading">"Account"</h2>
        <ul class="rows">
            <li class="row">
                <span class="row-link">
                    <span class="row-bar" data-state="running"></span>
                    <span class="row-name">{user.username}</span>
                    <span class="row-detail">"Signed in"</span>
                </span>
            </li>
            <li class="row">
                <a class="row-link" href="/accounts">
                    <span class="row-bar" data-state="running"></span>
                    <span class="row-name">"Accounts and password"</span>
                    <span class="row-detail">"Who can sign in, and your own password"</span>
                </a>
            </li>
            <li class="row">
                <a class="row-link" href="/tokens">
                    <span class="row-bar" data-state="running"></span>
                    <span class="row-name">"API tokens"</span>
                    <span class="row-detail">"Let another program act for you, within limits"</span>
                </a>
            </li>
        </ul>

        <h2 class="group-heading">"Host"</h2>
        {move || match info.get() {
            Load::Loading => view! { <p class="state-note">"Reading host"</p> }.into_any(),
            Load::Failed(message) => view! {
                <div class="state-note">
                    <p>"Could not read the host."</p>
                    <p>{message}</p>
                </div>
            }
            .into_any(),
            Load::Ready(detail) => {
                let reachable = detail.unreachable_reason.is_none();
                let version = detail
                    .server_version
                    .clone()
                    .unwrap_or_else(|| "unavailable".to_owned());
                let counts = format!(
                    "{} of {} containers running, {} images",
                    detail.containers_running, detail.containers_total, detail.images,
                );
                view! {
                    <ul class="rows">
                        <li class="row">
                            <span class="row-link">
                                <span
                                    class="row-bar"
                                    data-state=if reachable { "running" } else { "unhealthy" }
                                ></span>
                                <span class="row-name">{detail.name.clone()}</span>
                                <span class="row-detail">
                                    {detail
                                        .unreachable_reason
                                        .clone()
                                        .unwrap_or_else(|| format!("Docker {version}"))}
                                </span>
                            </span>
                        </li>
                    </ul>
                    <p class="verdict-count">{counts}</p>
                }
                .into_any()
            }
        }}

        <h2 class="group-heading">"Sources"</h2>
        <ul class="rows">
            <li class="row">
                <a class="row-link" href="/sources">
                    <span class="row-bar" data-state="running"></span>
                    <span class="row-name">"Repositories and credentials"</span>
                    <span class="row-detail">"Where Git-backed stacks come from"</span>
                </a>
            </li>
        </ul>

        <h2 class="group-heading">"Maintenance"</h2>
        <ul class="rows">
            <li class="row">
                <a class="row-link" href="/cleanup">
                    <span class="row-bar" data-state="stopped"></span>
                    <span class="row-name">"Reclaim disk space"</span>
                    <span class="row-detail">"Images no container is using"</span>
                </a>
            </li>
        </ul>

        <h2 class="group-heading">"Activity"</h2>
        <ul class="rows">
            <li class="row">
                <a class="row-link" href="/activity">
                    <span class="row-bar" data-state="running"></span>
                    <span class="row-name">"What has been done"</span>
                    <span class="row-detail">"Sign-ins, deploys, and changes"</span>
                </a>
            </li>
        </ul>

        <h2 class="group-heading">"Session"</h2>
        <Show when=move || sign_out_error.get().is_some()>
            <p class="notice" role="alert">{move || sign_out_error.get().unwrap_or_default()}</p>
        </Show>
        <button class="button" type="button" disabled=move || signing_out.get() on:click=sign_out>
            {move || if signing_out.get() { "Signing out" } else { "Sign out" }}
        </button>
    }
}
