//! What has been done, and by whom.

use leptos::prelude::*;
use shared::audit::AuditEntry;

use crate::api;
use crate::load::Load;
use crate::screen::Screen;
use crate::ui::Row;

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
                            view! { <Row state name=entry.action detail count=when /> }
                        })
                        .collect_view()}
                </ul>
            }
            .into_any(),
        }}
    }
}
