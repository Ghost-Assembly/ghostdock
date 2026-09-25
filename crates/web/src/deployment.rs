//! What one operation actually printed.

use leptos::prelude::*;
use shared::deployment::{DeploymentDetail, DeploymentStatus};
use shared::event::ServerEvent;

use crate::api;
use crate::events::use_events;
use crate::load::Load;
use crate::screen::Screen;
use crate::status::Outcome;
use crate::ui::{Topbar, route_id};

#[component]
pub fn DeploymentView() -> impl IntoView {
    let id = route_id();
    let detail = RwSignal::new(Load::<DeploymentDetail>::Loading);
    let screen = Screen::new();
    // Only the newest read may land: one started before the operation
    // finished would put back an older, shorter log.
    let latest = StoredValue::new(0_u64);

    let refresh = move || {
        let mine = latest.get_value() + 1;
        latest.set_value(mine);
        let wanted = id.get_untracked();
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
            } if *deployment_id == id.get_untracked() => {
                detail.update(|d| {
                    if let Load::Ready(d) = d
                        && d.deployment.status == DeploymentStatus::Running
                    {
                        if !d.log.is_empty() && !d.log.ends_with('\n') {
                            d.log.push('\n');
                        }
                        d.log.push_str(line);
                    }
                });
            }
            // The stored log is the whole of it, so the ending is read
            // rather than pieced together.
            ServerEvent::DeploymentFinished { deployment }
                if deployment.id == id.get_untracked() =>
            {
                refresh();
            }
            _ => {}
        });
    }

    // Back to the stack it ran on, once that is known.
    let back = Memo::new(move |_| {
        detail.with(|d| {
            d.ready().map_or_else(
                || "/".to_owned(),
                |d| format!("/stacks/{}", d.deployment.stack_id),
            )
        })
    });

    view! {
        <Topbar title="Output" back />

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
                let outcome = Outcome::of(&d.deployment);
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
                        <p class="verdict-line" data-tone=outcome.tone>{outcome.heading()}</p>
                        <p class="verdict-count">{when}</p>
                    </section>
                    <pre class="log" aria-live="polite" tabindex="0">{log}</pre>
                }
                .into_any()
            }
        }}
    }
}
