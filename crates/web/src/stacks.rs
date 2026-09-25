//! The status board.
//!
//! Exceptions first. A plain-language verdict answers "is everything fine?",
//! then anything needing attention, then the quiet majority. Sorting by name
//! instead would bury the one row that matters among forty that do not.

use leptos::prelude::*;
use shared::stack::{Stack, StackState};

use std::time::Duration;

use shared::event::ServerEvent;
use shared::metrics::{Now, StackNow};

use crate::api;
use crate::charts::Sparkline;
use crate::events::use_events;
use crate::load::Load;
use crate::screen::Screen;
use crate::status::usage;
use crate::ui::{Icon, Row, Topbar};

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

/// Serialised form used by the CSS to draw the state bar.
fn state_key(state: StackState) -> &'static str {
    match state {
        StackState::Running => "running",
        StackState::Degraded => "degraded",
        StackState::Unhealthy => "unhealthy",
        StackState::Stopped | StackState::Empty => "stopped",
    }
}

/// Which of its states the board is in. The stacks themselves are read
/// separately, so a reload that changes one row does not rebuild the rest.
#[derive(Clone, PartialEq)]
enum Shown {
    Loading,
    Failed(String),
    Empty,
    Board,
}

#[component]
pub fn Stacks() -> impl IntoView {
    let load = RwSignal::new(Load::<Vec<Stack>>::Loading);
    // Problems with how GhostDock itself is deployed. Shown on the board rather
    // than only in the logs, because the ones found make deploys fail
    // silently and a warning nobody reads protects nobody.
    let problems = RwSignal::new(Vec::<String>::new());
    // What each running stack is using now, refreshed every 5 s.
    let figures = RwSignal::new(None::<Now>);

    // Reloads in place: the rows already on screen stay until the new ones
    // arrive, so a live update never flashes the loading state. Reloads
    // overlap when events come quickly; only the newest may land, or an
    // older answer arriving late would put back what has since changed.
    // A 401 is handled once, for every screen, in the API client.
    let screen = Screen::new();
    let latest = StoredValue::new(0_u64);
    let refresh = move || {
        let mine = latest.get_value() + 1;
        latest.set_value(mine);
        screen.load(async move {
            let answer = api::stacks().await;
            if latest.try_get_value() != Some(mine) {
                return;
            }
            load.set(match answer {
                Ok(mut stacks) => {
                    stacks.sort_by(|a, b| {
                        (severity(a.state), &a.project).cmp(&(severity(b.state), &b.project))
                    });
                    Load::Ready(stacks)
                }
                Err(e) => Load::Failed(e.message),
            });
        });
    };
    // Found once, at startup, so read once here rather than on every
    // container event; again only after a reconnect, which may be a restart.
    let read_problems = move || {
        screen.load(async move {
            if let Ok(info) = api::host_info().await {
                problems.set(info.problems);
            }
        });
    };
    refresh();
    read_problems();
    screen.load(async move {
        if let Ok(now) = api::metrics_now().await {
            figures.set(Some(now));
        }
    });

    // Anything that changes a container, whoever did it, or a deploy ending,
    // is a reason to look again.
    if let Some(events) = use_events() {
        let reload = screen.coalesce(Duration::from_millis(400), refresh);
        events.on(move |event| match event {
            ServerEvent::ContainerChanged { .. } | ServerEvent::DeploymentFinished { .. } => {
                reload();
            }
            ServerEvent::Metrics { now } => figures.set(Some((**now).clone())),
            _ => {}
        });
        events.on_reconnect(move || {
            refresh();
            read_problems();
        });
        events.watch_metrics();
    }

    let shown = Memo::new(move |_| {
        load.with(|l| match l {
            Load::Loading => Shown::Loading,
            Load::Failed(message) => Shown::Failed(message.clone()),
            Load::Ready(stacks) if stacks.is_empty() => Shown::Empty,
            Load::Ready(_) => Shown::Board,
        })
    });
    let stacks = Memo::new(move |_| load.with(|l| l.ready().cloned().unwrap_or_default()));

    view! {
        <Topbar title="Stacks">
            <a class="topbar-link" href="/stacks/new"><Icon name="plus" />"New stack"</a>
        </Topbar>

        {move || {
            problems
                .get()
                .into_iter()
                .map(|problem| view! { <p class="notice" role="alert">{problem}</p> })
                .collect_view()
        }}

        {move || match shown.get() {
            Shown::Loading => view! { <p class="state-note">"Reading containers"</p> }.into_any(),
            Shown::Failed(message) => view! {
                <div class="state-note">
                    <p>"Could not read your stacks."</p>
                    <p>{message}</p>
                </div>
            }
            .into_any(),
            Shown::Empty => view! {
                <div class="state-note">
                    <p>"No stacks yet."</p>
                    <p>
                        "Add one with a compose file, or anything you deploy with Compose on this host will show up here."
                    </p>
                    <p><a class="button" href="/stacks/new">"New stack"</a></p>
                </div>
            }
            .into_any(),
            Shown::Board => view! { <Board stacks figures /> }.into_any(),
        }}
    }
}

/// The verdict line, its tone, and the counts beneath it.
fn verdict(stacks: &[Stack]) -> (String, &'static str, String) {
    let attention: Vec<&Stack> = stacks.iter().filter(|s| needs_attention(s.state)).collect();
    let containers: usize = stacks.iter().map(|s| s.total_count).sum();
    let running: usize = stacks.iter().map(|s| s.running_count).sum();
    let count_of = |state| stacks.iter().filter(|s| s.state == state).count();
    let stopped = count_of(StackState::Stopped);
    // Registered but never deployed: not running, and not a problem either.
    let undeployed = count_of(StackState::Empty);

    let (line, tone) = if !attention.is_empty() {
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
    (line, tone, counts.join(", "))
}

/// What one row shows, and so what identifies it: a row whose content is
/// unchanged by a reload is left exactly as it is, sparkline and all.
#[derive(Clone, PartialEq, Eq, Hash)]
struct RowData {
    project: String,
    /// Only a managed stack has anything to open.
    id: Option<i64>,
    detail: String,
    count: String,
    state: &'static str,
}

impl RowData {
    fn of(stack: &Stack) -> Self {
        let detail = if stack.managed.as_ref().is_some_and(|m| m.busy) {
            // Still says how it is, beside the bar that shows it: a deploy
            // under way does not make a stack healthy or unhealthy.
            format!("{}, working", state_word(stack.state))
        } else if stack.managed.is_none() {
            // Says why there is nothing to tap, rather than leaving a dead row.
            format!("{}, not managed by GhostDock", state_word(stack.state))
        } else {
            state_word(stack.state).to_owned()
        };
        Self {
            project: stack.project.clone(),
            id: stack.managed.as_ref().map(|m| m.id),
            detail,
            count: format!("{}/{}", stack.running_count, stack.total_count),
            state: state_key(stack.state),
        }
    }
}

#[component]
fn Board(stacks: Memo<Vec<Stack>>, figures: RwSignal<Option<Now>>) -> impl IntoView {
    let rows = move |attention: bool| {
        Memo::new(move |_| {
            stacks.with(|all| {
                all.iter()
                    .filter(|s| needs_attention(s.state) == attention)
                    .map(RowData::of)
                    .collect::<Vec<_>>()
            })
        })
    };
    let (attention, settled) = (rows(true), rows(false));
    let summary = Memo::new(move |_| stacks.with(|all| verdict(all)));

    view! {
        <section class="verdict">
            <p class="verdict-line" data-tone=move || summary.with(|s| s.1)>
                {move || summary.with(|s| s.0.clone())}
            </p>
            <p class="verdict-count">{move || summary.with(|s| s.2.clone())}</p>
        </section>

        <Show when=move || attention.with(|a| !a.is_empty())>
            <h2 class="group-heading">"Needs attention"</h2>
            <StackRows rows=attention figures />
        </Show>

        <Show when=move || settled.with(|s| !s.is_empty())>
            <h2 class="group-heading">
                {move || if attention.with(Vec::is_empty) { "All stacks" } else { "Everything else" }}
            </h2>
            <StackRows rows=settled figures />
        </Show>
    }
}

#[component]
fn StackRows(rows: Memo<Vec<RowData>>, figures: RwSignal<Option<Now>>) -> impl IntoView {
    view! {
        // Flows into columns where there is room: a board is scanned, not read.
        <ul class="rows rows-board">
            <For each=move || rows.get() key=|row| row.clone() let:row>
                <Row
                    state=row.state
                    name=row.project.clone()
                    ident=true
                    href=row.id.map(|id| format!("/stacks/{id}"))
                    detail=row.detail
                    count=row.count
                >
                    <RowFigures figures project=row.project />
                </Row>
            </For>
        </ul>
    }
}

/// A running stack's CPU and memory now, and on a desktop its last hour of
/// CPU. Reads only its own stack from the snapshot, and redraws the
/// sparkline only when its hour changes, once a minute, rather than with
/// every 5 s figure.
#[component]
fn RowFigures(figures: RwSignal<Option<Now>>, project: String) -> impl IntoView {
    fn find<'a>(now: &'a Option<Now>, project: &str) -> Option<&'a StackNow> {
        now.as_ref()?.stacks.iter().find(|s| s.project == project)
    }
    let project = StoredValue::new(project);
    // Present once the stack is in the figures, even with nothing measured.
    let now = Memo::new(move |_| {
        figures.with(|n| {
            project.with_value(|p| {
                find(n, p).map(|s| usage(s.reading.cpu, s.reading.mem).unwrap_or_default())
            })
        })
    });
    let hour = Memo::new(move |_| {
        figures.with(|n| project.with_value(|p| find(n, p).map(|s| s.cpu_hour.clone())))
    });

    view! {
        <Show when=move || now.with(Option::is_some)>
            <span class="row-figures">{move || now.get().unwrap_or_default()}</span>
            {move || hour.get().map(|values| view! { <Sparkline values /> })}
        </Show>
    }
}

fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}
