//! Account and host details.

use leptos::prelude::*;
use shared::auth::User;
use shared::host::HostInfo;

use crate::api;
use crate::app::Session;
use crate::load::Load;
use crate::screen::Screen;
use crate::ui::{ErrorNotice, Row};

#[component]
pub fn Settings(user: User, session: RwSignal<Session>) -> impl IntoView {
    let info = RwSignal::new(Load::<HostInfo>::Loading);
    let screen = Screen::new();
    let signing_out = RwSignal::new(false);
    let sign_out_error = RwSignal::new(None::<String>);

    screen.load(async move {
        info.set(Load::from(api::host_info().await));
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
            <Row state="running" name=user.username detail="Signed in" />
            <Row
                state="running"
                href="/accounts"
                name="Accounts and password"
                detail="Who can sign in, and your own password"
            />
            <Row
                state="running"
                href="/tokens"
                name="API tokens"
                detail="Let another program act for you, within limits"
            />
            <Row
                state="running"
                href="/settings/reference"
                name="API reference"
                detail="Every endpoint and MCP tool, and the permission each needs"
            />
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
                        <Row
                            state=if reachable { "running" } else { "unhealthy" }
                            name=detail.name
                            detail=detail
                                .unreachable_reason
                                .unwrap_or_else(|| format!("Docker {version}"))
                        />
                    </ul>
                    <p class="verdict-count">{counts}</p>
                }
                .into_any()
            }
        }}

        <h2 class="group-heading">"Sources"</h2>
        <ul class="rows">
            <Row
                state="running"
                href="/sources"
                name="Repositories and credentials"
                detail="Where Git-backed stacks come from"
            />
        </ul>

        <h2 class="group-heading">"Maintenance"</h2>
        <ul class="rows">
            <Row
                state="stopped"
                href="/cleanup"
                name="Reclaim disk space"
                detail="Images no container is using"
            />
        </ul>

        <h2 class="group-heading">"Activity"</h2>
        <ul class="rows">
            <Row
                state="running"
                href="/activity"
                name="What has been done"
                detail="Sign-ins, deploys, and changes"
            />
        </ul>

        <h2 class="group-heading">"Session"</h2>
        <ErrorNotice error=sign_out_error />
        <button class="button" type="button" disabled=move || signing_out.get() on:click=sign_out>
            {move || if signing_out.get() { "Signing out" } else { "Sign out" }}
        </button>
    }
}
