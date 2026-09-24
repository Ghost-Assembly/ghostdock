//! What one operation actually printed.

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use shared::deployment::{DeploymentDetail, DeploymentStatus};

use crate::api;
use crate::screen::Screen;

#[component]
pub fn DeploymentView() -> impl IntoView {
    let params = use_params_map();
    let detail = RwSignal::new(None::<DeploymentDetail>);
    let error = RwSignal::new(None::<String>);
    let screen = Screen::new();

    Effect::new(move |_| {
        let Some(id) = params.get().get("id").and_then(|v| v.parse::<i64>().ok()) else {
            return;
        };
        screen.load(async move {
            match api::deployment(id).await {
                Ok(d) => detail.set(Some(d)),
                Err(e) => error.set(Some(e.message)),
            }
        });
    });

    view! {
        <header class="topbar">
            <h1 class="wordmark">"Output"</h1>
            <a class="topbar-link" href="/">"Back"</a>
        </header>

        <Show when=move || error.get().is_some()>
            <p class="notice" role="alert">{move || error.get().unwrap_or_default()}</p>
        </Show>

        {move || match detail.get() {
            None => view! { <p class="state-note">"Loading"</p> }.into_any(),
            Some(d) => {
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
                let when = d.deployment.started_at.format("%d %b %Y, %H:%M").to_string();
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
                    <pre class="log">{log}</pre>
                }
                .into_any()
            }
        }}
    }
}
