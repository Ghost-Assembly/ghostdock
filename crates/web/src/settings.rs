//! Account and host details.

use leptos::prelude::*;
use shared::auth::User;
use shared::host::HostInfo;

use crate::api;
use crate::app::Session;
use crate::screen::Screen;

#[component]
pub fn Settings(user: User, session: RwSignal<Session>) -> impl IntoView {
    let info = RwSignal::new(Option::<HostInfo>::None);
    let screen = Screen::new();

    screen.load(async move {
        if let Ok(hosts) = api::hosts().await
            && let Some(host) = hosts.first()
            && let Ok(detail) = api::host_info(host.id).await
        {
            info.set(Some(detail));
        }
    });

    // The session is the whole app's, not this screen's, so it is updated
    // as part of the request: signing out lands wherever you have gone.
    let sign_out = move |_| {
        screen.act(
            async move {
                let _ = api::logout().await;
                session.set(Session::SignedOut);
            },
            |()| {},
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
            None => view! { <p class="state-note">"Reading host"</p> }.into_any(),
            Some(detail) => {
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
        <button class="button" type="button" on:click=sign_out>
            "Sign out"
        </button>
    }
}
