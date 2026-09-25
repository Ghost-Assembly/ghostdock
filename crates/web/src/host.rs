//! The host: what it is using now and over time, its disks and networks,
//! and the containers using the most.

use leptos::prelude::*;
use shared::event::ServerEvent;
use shared::metrics::{
    Current, Now, Range, Recommendation, Target, format_bytes, format_cores, format_rate,
};

use crate::api;
use crate::charts::{Measure, UsageBar};
use crate::events::use_events;
use crate::load::Load;
use crate::resources::{Charts, RangePicker, SizingRows};
use crate::screen::Screen;

const TOP: usize = 5;

#[component]
pub fn Host() -> impl IntoView {
    let screen = Screen::new();
    let now = RwSignal::new(None::<Now>);
    let error = RwSignal::new(None::<String>);
    let range = RwSignal::new(Range::Hour);
    let advice = RwSignal::new(Load::<Vec<Recommendation>>::Loading);

    screen.load(async move {
        match api::metrics_now().await {
            Ok(n) => now.set(Some(n)),
            Err(e) => error.set(Some(e.message)),
        }
    });
    screen.load(async move {
        advice.set(Load::from(api::sizing().await));
    });
    if let Some(events) = use_events() {
        events.on(move |event| {
            if let ServerEvent::Metrics { now: fresh } = event {
                now.set(Some(*fresh));
            }
        });
    }

    let summary = move || {
        let n = now.get()?;
        let host = n.host?;
        let cpus = n.host_cpus.map(|c| format!(" of {c}")).unwrap_or_default();
        let cpu = host.cpu.map(|c| format!("{}{cpus}", format_cores(c)));
        let mem = match (host.mem, host.mem_limit) {
            (Some(used), Some(total)) => Some(format!(
                "{} of {} memory",
                format_bytes(used),
                format_bytes(total)
            )),
            _ => None,
        };
        // Only the figures there are: a missing one leaves no stray comma.
        let parts: Vec<String> = [cpu, mem].into_iter().flatten().collect();
        (!parts.is_empty()).then(|| parts.join(", "))
    };

    view! {
        <header class="topbar">
            <h1 class="wordmark">"Host"</h1>
        </header>
        <Show when=move || error.get().is_some()>
            <p class="notice" role="alert">{move || error.get().unwrap_or_default()}</p>
        </Show>
        <Show when=move || now.get().is_some_and(|n| n.containers_unavailable)>
            <p class="notice" role="alert">"The Docker daemon is not answering; container figures will return with it."</p>
        </Show>
        <p class="verdict-count">{move || summary().unwrap_or_else(|| "Reading the host".to_owned())}</p>

        <RangePicker range />
        <Charts target=Signal::derive(Target::host) range measures=&[Measure::Cpu, Measure::Memory, Measure::Load] />

        <h2 class="group-heading">"Disks"</h2>
        <ul class="rows">
            {move || now.get().map(|n| n.disks).unwrap_or_default().into_iter().map(|d| {
                let (used, total) = (d.reading.mem.unwrap_or(0), d.reading.mem_limit.unwrap_or(0));
                view! {
                    <li class="row row-usage">
                        <span class="row-name">{d.key.clone()}</span>
                        <span class="row-detail">{format!("{} of {}", format_bytes(used), format_bytes(total))}</span>
                        <UsageBar used total />
                    </li>
                }
            }).collect_view()}
        </ul>
        <p class="entry-note">"More disks appear when mounted read-only under /host/disks."</p>

        <Show when=move || now.get().is_some_and(|n| !n.networks.is_empty())>
            <h2 class="group-heading">"Networks"</h2>
            <ul class="rows">
                {move || now.get().map(|n| n.networks).unwrap_or_default().into_iter().map(|n| view! {
                    <li class="row">
                        <span class="row-link">
                            <span class="row-bar" data-state="running"></span>
                            <span class="row-name">{n.key.clone()}</span>
                            <span class="row-detail">{format!(
                                "in {}, out {}",
                                format_rate(n.reading.net_rx.unwrap_or(0.0)),
                                format_rate(n.reading.net_tx.unwrap_or(0.0)),
                            )}</span>
                        </span>
                    </li>
                }).collect_view()}
            </ul>
        </Show>

        <h2 class="group-heading">"Using the most CPU"</h2>
        {move || top_rows(now.get().map(|n| n.containers).unwrap_or_default(), |c| c.reading.cpu.unwrap_or(0.0), |c| format_cores(c.reading.cpu.unwrap_or(0.0)))}
        <h2 class="group-heading">"Using the most memory"</h2>
        {move || top_rows(now.get().map(|n| n.containers).unwrap_or_default(), |c| {
            #[allow(clippy::cast_precision_loss)]
            let m = c.reading.mem.unwrap_or(0) as f64;
            m
        }, |c| format_bytes(c.reading.mem.unwrap_or(0)))}
        <h2 class="group-heading">"Sizing"</h2>
        {move || match advice.get() {
            Load::Loading => view! { <p class="entry-note">"Reading advice"</p> }.into_any(),
            Load::Failed(message) => view! {
                <p class="entry-note">{format!("Could not read the advice. {message}")}</p>
            }
            .into_any(),
            Load::Ready(list) if list.is_empty() => view! {
                <p class="entry-note">"Advice appears once containers have 3 days of history."</p>
            }
            .into_any(),
            Load::Ready(list) => view! { <SizingRows list /> }.into_any(),
        }}
    }
}

fn top_rows(
    mut list: Vec<Current>,
    key: fn(&Current) -> f64,
    label: fn(&Current) -> String,
) -> impl IntoView {
    list.sort_by(|a, b| key(b).total_cmp(&key(a)));
    view! {
        <ul class="rows rows-top">
            {list.into_iter().take(TOP).map(|c| view! {
                <li class="row">
                    <a class="row-link" href=format!("/containers/{}/resources", c.key)>
                        <span class="row-bar" data-state="running"></span>
                        <span class="row-name">{c.key.clone()}</span>
                        <span class="row-detail">{c.project.clone().unwrap_or_default()}</span>
                        <span class="row-count">{label(&c)}</span>
                    </a>
                </li>
            }).collect_view()}
        </ul>
    }
}
