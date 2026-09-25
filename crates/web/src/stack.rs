//! One stack: what it is doing, what you can do to it, and what happened.

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use shared::container::Container;
use shared::deployment::{Action, Deployment, DeploymentStatus, RegisteredStack};
use shared::event::ServerEvent;
use shared::metrics::{Range, Target};
use shared::update::UpdateStatus;

use crate::api;
use crate::charts::Measure;
use crate::confirm::Confirm;
use crate::events::use_events;
use crate::load::Load;
use crate::resources::{Charts, ContainerFigureRows, RangePicker};
use crate::screen::Screen;

/// Lines kept in the live pane.
///
/// A pull of several images produces a lot of output, and an unbounded list
/// on a phone is a memory leak with a progress bar.
const MAX_LIVE_LINES: usize = 400;

#[component]
pub fn StackDetail() -> impl IntoView {
    let params = use_params_map();
    let navigate = use_navigate();
    let id = Memo::new(move |_| {
        params
            .get()
            .get("id")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or_default()
    });

    let stack = RwSignal::new(None::<RegisteredStack>);
    let resources_range = RwSignal::new(Range::Hour);
    // A memo, so the stack reloading on every container event does not
    // refetch its series when nothing about the target changed.
    let resources_project =
        Memo::new(move |_| stack.with(|s| s.as_ref().map(|s| s.slug.clone()).unwrap_or_default()));
    let resources_target = Memo::new(move |_| Target::Stack(resources_project.get()));
    let history = RwSignal::new(Load::<Vec<Deployment>>::Loading);
    let containers = RwSignal::new(Load::<Vec<Container>>::Loading);
    // Reading the stack failed; cleared by the next read that works.
    let load_error = RwSignal::new(None::<String>);
    // Something asked of the server failed.
    let error = RwSignal::new(None::<String>);
    // The operation running now, found by a read or announced by an event.
    let active = RwSignal::new(None::<i64>);
    // Asked for here and not yet answered.
    let starting = RwSignal::new(false);
    // Busy is what the server says is running, not a flag someone has to
    // remember to lower: a finish missed while offline cannot leave the
    // buttons saying "Working" for good.
    let busy = Memo::new(move |_| starting.get() || active.get().is_some());
    // Output of the operation currently running, newest last.
    let live = RwSignal::new(Vec::<String>::new());
    let update = RwSignal::new(None::<UpdateStatus>);
    let update_error = RwSignal::new(None::<String>);
    let auto_apply = RwSignal::new(false);
    let checking = RwSignal::new(false);
    let toggling = RwSignal::new(false);
    let forgetting = RwSignal::new(false);
    let screen = Screen::new();
    // Reads overlap: an event, a reconnect and an action can each start
    // one. Only the newest may land, or an answer from before a deploy
    // began would say nothing is running.
    let latest = StoredValue::new(0_u64);
    // The newest operation heard to have finished. A quick one can finish
    // before the answer to starting it arrives, and must not then be
    // followed as if it were still running.
    let finished = StoredValue::new(0_i64);

    // Reads for display: dropped if this screen is left mid-way.
    let refresh = move || {
        let mine = latest.get_value() + 1;
        latest.set_value(mine);
        let current = move || latest.try_get_value() == Some(mine);
        let stack_id = id.get_untracked();
        screen.load(async move {
            let found = api::stack(stack_id).await;
            if !current() {
                return;
            }
            match found {
                Ok(s) => {
                    stack.set(Some(s));
                    load_error.set(None);
                }
                Err(e) => load_error.set(Some(e.message)),
            }

            let board = api::stacks(1).await;
            if !current() {
                return;
            }
            // Without the stack there is no telling which row is its own.
            if let Some(slug) = stack.get_untracked().map(|s| s.slug) {
                containers.set(match board {
                    Ok(board) => Load::Ready(
                        board
                            .into_iter()
                            .find(|s| s.project == slug)
                            .map(|s| s.containers)
                            .unwrap_or_default(),
                    ),
                    Err(e) => Load::Failed(e.message),
                });
            }

            let updates = api::updates(1).await;
            if !current() {
                return;
            }
            if let Ok(list) = updates
                && let Some(mine) = list.into_iter().find(|u| u.stack.id == stack_id)
            {
                auto_apply.set(mine.auto_apply);
                update.set(Some(mine.status));
            }

            let deployments = api::deployments(stack_id).await;
            if !current() {
                return;
            }
            match deployments {
                Ok(list) => {
                    // An operation still running was started elsewhere, or
                    // is ours after a reload; either way the pane follows
                    // it. None running means none is, whatever was thought.
                    active.set(
                        list.iter()
                            .find(|d| d.status == DeploymentStatus::Running)
                            .map(|d| d.id),
                    );
                    history.set(Load::Ready(list));
                }
                Err(e) => history.set(Load::Failed(e.message)),
            }
        });
    };

    Effect::new(move |_| {
        let _ = id.get();
        refresh();
    });

    // Fold server events into this view's state, one callback per message.
    if let Some(events) = use_events() {
        let reload = screen.coalesce(std::time::Duration::from_millis(400), refresh);
        events.on_reconnect(refresh);
        events.on(move |event| match event {
            // One of this stack's containers changed, whoever caused it.
            ServerEvent::ContainerChanged { change }
                if change.project.is_some()
                    && change.project == stack.get_untracked().map(|s| s.slug) =>
            {
                reload();
            }
            ServerEvent::DeploymentStarted {
                stack_id,
                deployment_id,
                ..
            } if stack_id == id.get_untracked() => {
                active.set(Some(deployment_id));
                live.set(Vec::new());
                // Also drops any read already under way, which may have
                // been answered before this began.
                refresh();
            }
            ServerEvent::DeploymentOutput {
                deployment_id,
                line,
            } if Some(deployment_id) == active.get_untracked() => {
                live.update(|lines| {
                    lines.push(line);
                    if lines.len() > MAX_LIVE_LINES {
                        lines.remove(0);
                    }
                });
            }
            ServerEvent::DeploymentFinished { deployment }
                if deployment.stack_id == id.get_untracked() =>
            {
                finished.set_value(finished.get_value().max(deployment.id));
                if Some(deployment.id) == active.get_untracked() {
                    active.set(None);
                }
                refresh();
            }
            _ => {}
        });
    }

    let start = move |action: &'static str| {
        if busy.get_untracked() {
            return;
        }
        starting.set(true);
        error.set(None);
        live.set(Vec::new());
        // A read already under way may be answered from before this began.
        latest.set_value(latest.get_value() + 1);
        // The operation runs on the server whether or not anyone stays here.
        screen.act(api::act(id.get_untracked(), action), move |result| {
            match result {
                Ok(d) if d.id > finished.get_value() => active.set(Some(d.id)),
                Ok(_) => {}
                Err(e) => error.set(Some(e.message)),
            }
            starting.set(false);
        });
    };
    let run = move |action: &'static str| move |_| start(action);
    let take_down = Callback::new(move |()| start("down"));

    let forget = Callback::new(move |()| {
        if forgetting.get_untracked() {
            return;
        }
        forgetting.set(true);
        error.set(None);
        let navigate = navigate.clone();
        screen.act(
            api::delete_stack(id.get_untracked()),
            move |result| match result {
                Ok(()) => navigate("/", Default::default()),
                Err(e) => {
                    error.set(Some(e.message));
                    forgetting.set(false);
                }
            },
        );
    });

    let check = move |_| {
        if checking.get_untracked() {
            return;
        }
        checking.set(true);
        update_error.set(None);
        screen.act(api::check_stack(id.get_untracked()), move |result| {
            match result {
                Ok(status) => update.set(Some(status)),
                Err(e) => update_error.set(Some(e.message)),
            }
            checking.set(false);
        });
    };

    let toggle_auto_apply = move |_| {
        if toggling.get_untracked() {
            return;
        }
        toggling.set(true);
        update_error.set(None);
        let next = !auto_apply.get_untracked();
        screen.act(
            api::set_auto_apply(id.get_untracked(), next),
            move |result| {
                match result {
                    Ok(saved) => auto_apply.set(saved.enabled),
                    Err(e) => update_error.set(Some(e.message)),
                }
                toggling.set(false);
            },
        );
    };

    view! {
        <header class="topbar">
            <h1 class="wordmark">
                {move || stack.get().map_or_else(|| "Stack".to_owned(), |s| s.name)}
            </h1>
            <a class="topbar-link" href="/">"Back"</a>
        </header>

        <Show when=move || load_error.get().is_some()>
            <p class="notice" role="alert">{move || load_error.get().unwrap_or_default()}</p>
        </Show>
        <Show when=move || error.get().is_some()>
            <p class="notice" role="alert">{move || error.get().unwrap_or_default()}</p>
        </Show>

        // Two panes where there is room: what is happening on the left,
        // how it is set up on the right. One column, in this order, on a phone.
        <div class="detail">
        <div class="detail-main">
        <div class="actions">
            <button class="button" type="button" disabled=move || busy.get() on:click=run("deploy")>
                {move || if busy.get() { "Working" } else { "Deploy" }}
            </button>
            <button class="button button-quiet" type="button" disabled=move || busy.get()
                on:click=run("restart")>
                "Restart"
            </button>
            <button class="button button-quiet" type="button" disabled=move || busy.get()
                on:click=run("stop")>
                "Stop"
            </button>
        </div>

        <Show when=move || !live.get().is_empty()>
            <h2 class="group-heading">"Output"</h2>
            <pre class="log" aria-live="polite">
                {move || live.get().join("\n")}
            </pre>
        </Show>

        <h2 class="group-heading">"History"</h2>
        {move || match history.get() {
            Load::Loading => view! { <p class="state-note">"Loading"</p> }.into_any(),
            Load::Failed(message) => view! {
                <div class="state-note">
                    <p>"Could not read the history."</p>
                    <p>{message}</p>
                </div>
            }
            .into_any(),
            Load::Ready(list) if list.is_empty() => view! {
                <div class="state-note">
                    <p>"Nothing has run yet."</p>
                    <p>"Deploy to bring this stack up."</p>
                </div>
            }
            .into_any(),
            Load::Ready(list) => view! { <HistoryRows deployments=list /> }.into_any(),
        }}

        <h2 class="group-heading">"Containers"</h2>
        {move || match containers.get() {
            Load::Loading => view! { <p class="entry-note">"Reading containers"</p> }.into_any(),
            Load::Failed(message) => view! {
                <p class="entry-note">{format!("Could not read its containers. {message}")}</p>
            }
            .into_any(),
            Load::Ready(list) if list.is_empty() => view! {
                <p class="entry-note">"Nothing is running for this stack."</p>
            }
            .into_any(),
            Load::Ready(list) => view! { <ContainerRows containers=list /> }.into_any(),
        }}

        // Only once the stack is known: its name is the series' subject.
        <Show when=move || stack.with(Option::is_some)>
            <h2 class="group-heading">"Resources"</h2>
            <RangePicker range=resources_range />
            <Charts
                target=resources_target.into()
                range=resources_range
                measures=&[Measure::Cpu, Measure::Memory]
            />
            <ContainerFigureRows project=resources_project.into() range=resources_range />
        </Show>

        </div>
        <aside class="detail-side">
        <h2 class="group-heading">"Updates"</h2>
        <p class="entry-note">{move || update_summary(update.get().as_ref())}
        </p>
        <Show when=move || update_error.get().is_some()>
            <p class="notice" role="alert">{move || update_error.get().unwrap_or_default()}</p>
        </Show>
        <div class="actions actions-pair">
            <button
                class="button button-quiet"
                type="button"
                disabled=move || checking.get()
                on:click=check
            >
                {move || if checking.get() { "Checking" } else { "Check for updates" }}
            </button>
            <button
                class="button button-quiet"
                type="button"
                disabled=move || toggling.get()
                on:click=toggle_auto_apply
            >
                {move || if auto_apply.get() { "Auto-apply: on" } else { "Auto-apply: off" }}
            </button>
        </div>
        <p class="entry-note">
            "With auto-apply on, GhostDock deploys a change as soon as it finds one. \
             With it off, nothing happens until you say so."
        </p>

        <h2 class="group-heading">"Source"</h2>
        {move || match stack.get().and_then(|s| s.git) {
            None => view! {
                <p class="entry-note">"A compose file kept in GhostDock."</p>
            }
            .into_any(),
            Some(git) => {
                let commit = git.last_commit.clone().map_or_else(
                    || "never deployed".to_owned(),
                    |sha| format!("last deployed {}", shared::short(&sha, 12)),
                );
                view! {
                    <ul class="rows">
                        <li class="row">
                            <span class="row-link">
                                <span class="row-bar" data-state="running"></span>
                                <span class="row-name">{git.repo_url.clone()}</span>
                                <span class="row-detail">
                                    {format!("{} on {}", git.compose_path, git.git_ref)}
                                </span>
                            </span>
                        </li>
                    </ul>
                    <p class="entry-note">{commit}</p>
                }
                .into_any()
            }
        }}

        <h2 class="group-heading">"Environment"</h2>
        <a class="button button-quiet" href=move || format!("/stacks/{}/env", id.get())>
            "Edit variables"
        </a>

        <h2 class="group-heading">"Definition"</h2>
        <Show when=move || stack.get().is_some_and(|s| s.git.is_none())>
            <a class="button button-quiet" href=move || format!("/stacks/{}/edit", id.get())>
                "Edit compose file"
            </a>
        </Show>
        <Show when=move || stack.get().is_some_and(|s| s.git.is_some())>
            <p class="entry-note">
                "The compose file lives in the repository. Change it there and deploy."
            </p>
        </Show>

        <h2 class="group-heading">"Danger"</h2>
        <Confirm
            label="Take down"
            confirm="Take it down"
            disabled=Signal::derive(move || busy.get())
            on_confirm=take_down
        />
        <p class="entry-note">
            "Removes the containers and networks. Named volumes and the registration stay, \
             so Deploy brings it back with its data."
        </p>
        <Confirm
            label="Forget this stack"
            confirm="Forget it"
            disabled=Signal::derive(move || forgetting.get())
            on_confirm=forget
        />
        <p class="entry-note">
            "Removes it from GhostDock only. Whatever is running keeps running."
        </p>
        </aside>
        </div>
    }
}

/// Each container, with the way in to its output.
///
/// The logs link is the reason this list exists: when something is wrong,
/// the next thing anyone wants is what the container said about it.
#[component]
fn ContainerRows(containers: Vec<Container>) -> impl IntoView {
    view! {
        <ul class="rows">
            {containers
                .into_iter()
                .map(|container| {
                    let state = match container.state {
                        shared::container::ContainerState::Running
                        | shared::container::ContainerState::Restarting => "running",
                        _ => "stopped",
                    };
                    let state = if container.health
                        == Some(shared::container::Health::Unhealthy)
                        && container.state.is_running()
                    {
                        "unhealthy"
                    } else {
                        state
                    };
                    let detail = container.status.clone();
                    let href = format!("/containers/{}/logs", container.id);
                    let shell_href = format!("/containers/{}/shell", container.id);
                    let resources_href = format!("/containers/{}/resources", container.name);
                    view! {
                        <li class="row">
                            <a class="row-link" href=href>
                                <span class="row-bar" data-state=state></span>
                                <span class="row-name">{container.name.clone()}</span>
                                <span class="row-detail">{detail}</span>
                                <span class="row-count">"Logs"</span>
                            </a>
                            <a class="row-aside" href=shell_href>"Shell"</a>
                            <a class="row-aside" href=resources_href>"Resources"</a>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
}

#[component]
fn HistoryRows(deployments: Vec<Deployment>) -> impl IntoView {
    view! {
        <ul class="rows">
            {deployments
                .into_iter()
                .map(|d| {
                    let state = match d.status {
                        DeploymentStatus::Succeeded => "running",
                        DeploymentStatus::Failed => "unhealthy",
                        DeploymentStatus::Running => "degraded",
                    };
                    let outcome = match d.status {
                        DeploymentStatus::Succeeded => "succeeded".to_owned(),
                        DeploymentStatus::Running => "running now".to_owned(),
                        DeploymentStatus::Failed => d
                            .exit_code
                            .map_or_else(|| "failed".to_owned(), |c| format!("failed (exit {c})")),
                    };
                    let when = crate::time::local(d.started_at, "%d %b %H:%M");
                    let href = format!("/deployments/{}", d.id);
                    view! {
                        <li class="row">
                            <a class="row-link" href=href>
                                <span class="row-bar" data-state=state></span>
                                <span class="row-name">{action_label(d.action)}</span>
                                <span class="row-detail">{outcome}</span>
                                <span class="row-count">{when}</span>
                            </a>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
}

fn action_label(action: Action) -> &'static str {
    match action {
        Action::Deploy => "Deploy",
        Action::Stop => "Stop",
        Action::Restart => "Restart",
        Action::Remove => "Take down",
    }
}

/// One line describing what the last check found.
///
/// "Not checked yet" is kept distinct from "up to date": they look the same
/// on a screen that conflates them, and only one of them is reassuring.
fn update_summary(status: Option<&UpdateStatus>) -> String {
    let Some(status) = status else {
        return "Not checked yet.".to_owned();
    };
    if let Some(error) = &status.error {
        return error.clone();
    }
    if status.has_update() {
        "Something is waiting. Deploy to apply it.".to_owned()
    } else if status.checked_at.is_some() {
        "Up to date.".to_owned()
    } else {
        "Not checked yet.".to_owned()
    }
}
