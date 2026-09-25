//! What has been done, and by whom.

use leptos::prelude::*;
use shared::audit::AuditEntry;

use crate::api;
use crate::load::Load;
use crate::screen::Screen;

#[component]
pub fn Activity() -> impl IntoView {
    let entries = RwSignal::new(Load::<Vec<AuditEntry>>::Loading);
    let screen = Screen::new();

    Effect::new(move |_| {
        screen.load(async move {
            entries.set(Load::from(api::audit().await));
        });
    });

    view! {
        <header class="topbar">
            <h1 class="wordmark">"Activity"</h1>
            <a class="topbar-link" href="/settings">"Back"</a>
        </header>

        {move || match entries.get() {
            Load::Loading => view! { <p class="state-note">"Loading"</p> }.into_any(),
            Load::Failed(message) => view! {
                <div class="state-note">
                    <p>"Could not read the activity."</p>
                    <p>{message}</p>
                </div>
            }
            .into_any(),
            Load::Ready(list) if list.is_empty() => view! {
                <div class="state-note">
                    <p>"Nothing recorded yet."</p>
                </div>
            }
            .into_any(),
            Load::Ready(list) => view! {
                <ul class="rows">
                    {list
                        .into_iter()
                        .map(|entry| {
                            // Failures are the reason to read this at all, so
                            // they are the only rows that carry colour.
                            let state = if entry.action.starts_with("failed") {
                                "unhealthy"
                            } else {
                                "running"
                            };
                            let detail = format!("{} on {}", entry.username, entry.target);
                            let when = crate::time::local(entry.at, "%d %b %H:%M");
                            view! {
                                <li class="row">
                                    <span class="row-link">
                                        <span class="row-bar" data-state=state></span>
                                        <span class="row-name">{entry.action}</span>
                                        <span class="row-detail">{detail}</span>
                                        <span class="row-count">{when}</span>
                                    </span>
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
            }
            .into_any(),
        }}
    }
}
