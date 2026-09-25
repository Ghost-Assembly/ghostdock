//! What one operation actually printed.

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use shared::deployment::{DeploymentDetail, DeploymentStatus};
use shared::event::ServerEvent;

use crate::api;
use crate::events::use_events;
use crate::load::Load;
use crate::screen::Screen;

#[component]
pub fn DeploymentView() -> impl IntoView {
    let params = use_params_map();
    let id = Memo::new(move |_| params.get().get("id").and_then(|v| v.parse::<i64>().ok()));
    let detail = RwSignal::new(Load::<DeploymentDetail>::Loading);
    let screen = Screen::new();
    // Only the newest read may land: one started before the operation
    // finished would put back an older, shorter log.
    let latest = StoredValue::new(0_u64);

    let refresh = move || {
        let mine = latest.get_value() + 1;
        latest.set_value(mine);
        let Some(wanted) = id.get_untracked() else {
            detail.set(Load::Failed("There is no such operation.".to_owned()));
            return;
        };
        screen.load(async move {
            let answer = api::deployment(wanted).await;
            if latest.try_get_value() == Some(mine) {
                detail.set(Load::from(answer));
            }
        });
    };
    Effect::new(move |_| {
        let _ = id.get();
        refresh();
    });

    // An operation still running is followed as it goes, rather than
    // showing whatever it had printed when the page opened.
    if let Some(events) = use_events() {
        events.on_reconnect(refresh);
        events.on(move |event| match event {
            ServerEvent::DeploymentOutput {
                deployment_id,
                line,
            } if Some(deployment_id) == id.get_untracked() => {
                detail.update(|d| {
                    if let Load::Ready(d) = d
                        && d.deployment.status == DeploymentStatus::Running
                    {
                        if !d.log.is_empty() && !d.log.ends_with('\n') {
                            d.log.push('\n');
                        }
                        d.log.push_str(&line);
                    }
                });
            }
            // The stored log is the whole of it, so the ending is read
            // rather than pieced together.
            ServerEvent::DeploymentFinished { deployment }
                if Some(deployment.id) == id.get_untracked() =>
            {
                refresh();
            }
            _ => {}
        });
    }

    view! {
        <header class="topbar">
            <h1 class="wordmark">"Output"</h1>
            <a class="topbar-link" href="/">"Back"</a>
        </header>

        {move || match detail.get() {
            Load::Loading => view! { <p class="state-note">"Loading"</p> }.into_any(),
            Load::Failed(message) => view! {
                <div class="state-note">
                    <p>"Could not read this operation."</p>
                    <p>{message}</p>
                </div>
            }
            .into_any(),
            Load::Ready(d) => {
                let outcome = match d.deployment.status {
                    DeploymentStatus::Succeeded => "Succeeded".to_owned(),
                    DeploymentStatus::Running => "Still running".to_owned(),
                    DeploymentStatus::Failed => d
                        .deployment
                        .exit_code
                        .map_or_else(
                            || "Failed".to_owned(),
                            |c| format!("Failed with exit code {c}"),
                        ),
                };
                let tone = match d.deployment.status {
                    DeploymentStatus::Succeeded => "quiet",
                    DeploymentStatus::Running => "degraded",
                    DeploymentStatus::Failed => "bad",
                };
                let when = crate::time::local(d.deployment.started_at, "%d %b %Y, %H:%M");
                // Compose's own words, verbatim. A summary here would hide
                // the one line that explains what went wrong.
                let log = if d.log.trim().is_empty() {
                    "(no output)".to_owned()
                } else {
                    d.log
                };
                view! {
                    <section class="verdict">
                        <p class="verdict-line" data-tone=tone>{outcome}</p>
                        <p class="verdict-count">{when}</p>
                    </section>
                    <pre class="log" aria-live="polite">{log}</pre>
                }
                .into_any()
            }
        }}
    }
}
