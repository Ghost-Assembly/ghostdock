//! The status board.
//!
//! Exceptions first. A plain-language verdict answers "is everything fine?",
//! then anything needing attention, then the quiet majority. Sorting by name
//! instead would bury the one row that matters among forty that do not.

use leptos::prelude::*;
use shared::stack::{Stack, StackState};

use std::time::Duration;

use shared::event::ServerEvent;
use shared::metrics::{Now, format_bytes, format_cores};

use crate::api;
use crate::app::Session;
use crate::charts::Sparkline;
use crate::events::use_events;
use crate::screen::Screen;

#[derive(Clone, Debug, PartialEq)]
enum Load {
    Loading,
    Ready(Vec<Stack>),
    Failed(String),
}

/// How alarming a state is. Drives ordering, so the worst is read first.
fn severity(state: StackState) -> u8 {
    match state {
        StackState::Unhealthy => 0,
        StackState::Degraded => 1,
        StackState::Stopped => 2,
        StackState::Empty => 3,
        StackState::Running => 4,
    }
}

fn needs_attention(state: StackState) -> bool {
    matches!(state, StackState::Unhealthy | StackState::Degraded)
}

fn state_word(state: StackState) -> &'static str {
    match state {
        StackState::Running => "running",
        StackState::Degraded => "partly running",
        StackState::Stopped => "stopped",
        StackState::Unhealthy => "unhealthy",
        StackState::Empty => "no containers",
    }
}

/// Serialised form used by the CSS to colour the state bar.
fn state_key(state: StackState) -> &'static str {
    match state {
        StackState::Running => "running",
        StackState::Degraded => "degraded",
        StackState::Unhealthy => "unhealthy",
        StackState::Stopped | StackState::Empty => "stopped",
    }
}

#[component]
pub fn Stacks() -> impl IntoView {
    let load = RwSignal::new(Load::Loading);
    let session = use_context::<RwSignal<Session>>();
    // Problems with how GhostDock itself is deployed. Shown on the board rather
    // than only in the logs, because the ones found make deploys fail
    // silently and a warning nobody reads protects nobody.
    let problems = RwSignal::new(Vec::<String>::new());
    // What each running stack is using now, refreshed every 5 s.
    let figures = RwSignal::new(None::<Now>);

    /// Retires the session on a 401 so the shell falls back to sign-in.
    fn handle(error: &api::Error, load: RwSignal<Load>, session: Option<RwSignal<Session>>) {
        match session {
            Some(session) if error.is_unauthenticated() => session.set(Session::SignedOut),
            _ => load.set(Load::Failed(error.message.clone())),
        }
    }

    // Reloads in place: the rows already on screen stay until the new ones
    // arrive, so a live update never flashes the loading state.
    let screen = Screen::new();
    let refresh = move || {
        screen.load(async move {
            let hosts = match api::hosts().await {
                Ok(hosts) => hosts,
                Err(e) => {
                    handle(&e, load, session);
                    return;
                }
            };

            let Some(host) = hosts.first() else {
                load.set(Load::Ready(Vec::new()));
                return;
            };

            if let Ok(info) = api::host_info(host.id).await {
                problems.set(info.problems);
            }

            match api::stacks(host.id).await {
                Ok(mut stacks) => {
                    stacks.sort_by_key(|s| (severity(s.state), s.project.clone()));
                    load.set(Load::Ready(stacks));
                }
                Err(e) => handle(&e, load, session),
            }
        });
    };
    refresh();
    screen.load(async move {
        if let Ok(now) = api::metrics_now().await {
            figures.set(Some(now));
        }
    });

    // Anything that changes a container, whoever did it, or a deploy ending,
    // is a reason to look again.
    if let Some(events) = use_events() {
        let reload = screen.coalesce(Duration::from_millis(400), refresh);
        events.on(move |event| {
            if matches!(
                event,
                ServerEvent::ContainerChanged { .. } | ServerEvent::DeploymentFinished { .. }
            ) {
                reload();
            }
        });
        events.on_reconnect(refresh);
        events.watch_metrics();
        events.on(move |event| {
            if let ServerEvent::Metrics { now } = event {
                figures.set(Some(*now));
            }
        });
    }

    view! {
        <header class="topbar">
            <h1 class="wordmark">"Stacks"</h1>
            <a class="topbar-link" href="/stacks/new">"New stack"</a>
        </header>

        {move || {
            problems
                .get()
                .into_iter()
                .map(|problem| view! { <p class="notice" role="alert">{problem}</p> })
                .collect_view()
        }}

        {move || match load.get() {
            Load::Loading => view! { <p class="state-note">"Reading containers"</p> }.into_any(),
            Load::Failed(message) => view! {
                <div class="state-note">
                    <p>"Could not read your stacks."</p>
                    <p>{message}</p>
                </div>
            }
            .into_any(),
            Load::Ready(stacks) if stacks.is_empty() => view! {
                <div class="state-note">
                    <p>"No stacks yet."</p>
                    <p>
                        "Add one with a compose file, or anything you deploy with Compose on this host will show up here."
                    </p>
                    <p><a class="button" href="/stacks/new">"New stack"</a></p>
                </div>
            }
            .into_any(),
            Load::Ready(stacks) => view! { <Board stacks figures /> }.into_any(),
        }}
    }
}

#[component]
fn Board(stacks: Vec<Stack>, figures: RwSignal<Option<Now>>) -> impl IntoView {
    let attention: Vec<Stack> = stacks
        .iter()
        .filter(|s| needs_attention(s.state))
        .cloned()
        .collect();
    let settled: Vec<Stack> = stacks
        .iter()
        .filter(|s| !needs_attention(s.state))
        .cloned()
        .collect();

    let has_attention = !attention.is_empty();
    let has_settled = !settled.is_empty();
    let settled_heading = if has_attention {
        "Everything else"
    } else {
        "All stacks"
    };

    let containers: usize = stacks.iter().map(|s| s.total_count).sum();
    let running: usize = stacks.iter().map(|s| s.running_count).sum();
    let count_of = |state| stacks.iter().filter(|s| s.state == state).count();
    let stopped = count_of(StackState::Stopped);
    // Registered but never deployed: not running, and not a problem either.
    let undeployed = count_of(StackState::Empty);

    let (verdict, tone) = if !attention.is_empty() {
        let n = attention.len();
        let tone = if attention.iter().any(|s| s.state == StackState::Unhealthy) {
            "bad"
        } else {
            "degraded"
        };
        (
            if n == 1 {
                "1 stack needs attention".to_owned()
            } else {
                format!("{n} stacks need attention")
            },
            tone,
        )
    } else if running == 0 {
        ("Nothing is running".to_owned(), "quiet")
    } else if stopped > 0 {
        // "Everything is running" would not be true, and nothing is wrong.
        ("Nothing needs attention".to_owned(), "quiet")
    } else if undeployed > 0 {
        ("Everything deployed is running".to_owned(), "quiet")
    } else {
        ("Everything is running".to_owned(), "quiet")
    };

    let mut counts = vec![
        format!(
            "{} {}",
            stacks.len(),
            plural(stacks.len(), "stack", "stacks")
        ),
        format!(
            "{containers} {}",
            plural(containers, "container", "containers")
        ),
    ];
    if stopped > 0 {
        counts.push(format!("{stopped} stopped"));
    }
    if undeployed > 0 {
        counts.push(format!("{undeployed} not deployed"));
    }
    let count_line = counts.join(", ");

    view! {
        <section class="verdict">
            <p class="verdict-line" data-tone=tone>{verdict}</p>
            <p class="verdict-count">{count_line}</p>
        </section>

        <Show when=move || has_attention>
            <h2 class="group-heading">"Needs attention"</h2>
            <StackRows stacks=attention.clone() figures />
        </Show>

        <Show when=move || has_settled>
            <h2 class="group-heading">{settled_heading}</h2>
            <StackRows stacks=settled.clone() figures />
        </Show>
    }
}

#[component]
fn StackRows(stacks: Vec<Stack>, figures: RwSignal<Option<Now>>) -> impl IntoView {
    view! {
        // Flows into columns where there is room: a board is scanned, not read.
        <ul class="rows rows-board">
            {stacks
                .into_iter()
                .map(|stack| {
                    let detail = if stack.managed.as_ref().is_some_and(|m| m.busy) {
                        "working".to_owned()
                    } else if stack.managed.is_none() {
                        // Says why there is nothing to tap, rather than
                        // leaving a dead row.
                        format!("{}, not managed by GhostDock", state_word(stack.state))
                    } else {
                        state_word(stack.state).to_owned()
                    };
                    let count = format!("{}/{}", stack.running_count, stack.total_count);
                    let bar = state_key(stack.state);
                    let project = stack.project.clone();

                    // Only a managed stack has anything to open.
                    match stack.managed.as_ref().map(|m| m.id) {
                        Some(id) => view! {
                            <li class="row">
                                <a class="row-link" href=format!("/stacks/{id}")>
                                    <span class="row-bar" data-state=bar></span>
                                    <span class="row-name">{project.clone()}</span>
                                    <span class="row-detail">{detail}</span>
                                    {row_figures(figures, project.clone())}
                                    <span class="row-count">{count}</span>
                                </a>
                            </li>
                        }
                        .into_any(),
                        None => view! {
                            <li class="row">
                                <span class="row-link">
                                    <span class="row-bar" data-state=bar></span>
                                    <span class="row-name">{project.clone()}</span>
                                    <span class="row-detail">{detail}</span>
                                    {row_figures(figures, project.clone())}
                                    <span class="row-count">{count}</span>
                                </span>
                            </li>
                        }
                        .into_any(),
                    }
                })
                .collect_view()}
        </ul>
    }
}

/// A running stack's CPU and memory now, and on a desktop its last hour of
/// CPU. Reads only its own stack from the snapshot: forty rows each cloning
/// the whole of it every 5 s would be waste.
fn row_figures(figures: RwSignal<Option<Now>>, project: String) -> impl IntoView {
    move || {
        figures
            .with(|n| {
                n.as_ref()?
                    .stacks
                    .iter()
                    .find(|s| s.project == project)
                    .cloned()
            })
            .map(|s| {
                let parts: Vec<String> = [
                    s.reading.cpu.map(format_cores),
                    s.reading.mem.map(format_bytes),
                ]
                .into_iter()
                .flatten()
                .collect();
                view! {
                    <span class="row-figures">{parts.join(", ")}</span>
                    <Sparkline values=s.cpu_hour />
                }
            })
    }
}

fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}
