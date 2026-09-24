//! What has been done, and by whom.

use leptos::prelude::*;
use shared::audit::AuditEntry;

use crate::api;
use crate::screen::Screen;

#[component]
pub fn Activity() -> impl IntoView {
    let entries = RwSignal::new(None::<Vec<AuditEntry>>);
    let error = RwSignal::new(None::<String>);
    let screen = Screen::new();

    Effect::new(move |_| {
        screen.load(async move {
            match api::audit().await {
                Ok(list) => entries.set(Some(list)),
                Err(e) => error.set(Some(e.message)),
            }
        });
    });

    view! {
        <header class="topbar">
            <h1 class="wordmark">"Activity"</h1>
            <a class="topbar-link" href="/settings">"Back"</a>
        </header>

        <Show when=move || error.get().is_some()>
            <p class="notice" role="alert">{move || error.get().unwrap_or_default()}</p>
        </Show>

        {move || match entries.get() {
            None => view! { <p class="state-note">"Loading"</p> }.into_any(),
            Some(list) if list.is_empty() => view! {
                <div class="state-note">
                    <p>"Nothing recorded yet."</p>
                </div>
            }
            .into_any(),
            Some(list) => view! {
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
                            let when = entry.at.format("%d %b %H:%M").to_string();
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
