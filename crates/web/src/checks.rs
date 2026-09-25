//! Uptime checks: the list on the Host screen and on a stack's page, a
//! check's own page, and the form that adds or changes one.

use std::time::Duration;

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_query_map};
use shared::checks::{
    Check, CheckInput, CheckKind, CheckPoint, CheckState, CheckSummary, Incident, format_uptime,
};
use shared::event::ServerEvent;
use shared::metrics::{Range, Reading};

use crate::api;
use crate::charts::{Chart, Measure, Sparkline};
use crate::confirm::Confirm;
use crate::events::use_events;
use crate::resources::RangePicker;
use crate::screen::Screen;
use crate::ui::{ErrorNotice, Pick, Row, TextField, Tick, Topbar, came_from, options, route_id};

/// The bar and mark a check's state is drawn with.
#[must_use]
pub fn state_key(state: CheckState) -> &'static str {
    match state {
        CheckState::Up => "running",
        CheckState::Degraded => "degraded",
        CheckState::Down => "unhealthy",
        CheckState::Pending => "busy",
        CheckState::Paused => "stopped",
    }
}

/// What a row says of a check: its state and why, its latest latency, and
/// its uptime once it has any.
fn detail(s: &CheckSummary) -> String {
    let status = &s.status;
    let mut parts = vec![match &status.message {
        Some(why) => format!("{}: {why}", status.state.as_str()),
        None => status.state.as_str().to_owned(),
    }];
    if let Some(ms) = status.latency_ms {
        parts.push(format!("{ms} ms"));
    }
    if s.uptime_24h.is_some() {
        parts.push(format!(
            "{} over 24 h, {} over 30 d",
            format_uptime(s.uptime_24h),
            format_uptime(s.uptime_30d)
        ));
    }
    parts.join(", ")
}

/// How long something lasted, as a person says it.
fn lasted(secs: i64) -> String {
    match secs {
        s if s < 60 => "under a minute".to_owned(),
        s if s < 3_600 => format!("{} min", s / 60),
        s if s < 86_400 => format!("{} h {} min", s / 3_600, s % 3_600 / 60),
        s => format!("{} days", s / 86_400),
    }
}

/// A chart point: latency in the CPU fields, uptime in percent as load.
#[allow(clippy::cast_precision_loss)]
fn reading(p: &CheckPoint) -> Reading {
    Reading {
        t: p.t,
        cpu: p.latency_avg,
        cpu_max: p.latency_max,
        load: (p.total > 0).then(|| f64::from(p.up) * 100.0 / f64::from(p.total)),
        ..Reading::default()
    }
}

/// A stack's checks, or with no stack every check on the host, and the way
/// to add one.
#[component]
pub fn CheckList(#[prop(optional, into)] stack: Option<Signal<i64>>) -> impl IntoView {
    let screen = Screen::new();
    let list = RwSignal::new(Vec::<CheckSummary>::new());
    // Said instead of the list while there is none to show.
    let note = RwSignal::new(Some("Reading checks".to_owned()));
    let latest = StoredValue::new(0_u64);
    let refresh = move || {
        let mine = latest.get_value() + 1;
        latest.set_value(mine);
        screen.load(async move {
            let answer = api::checks().await;
            if latest.try_get_value() != Some(mine) {
                return;
            }
            match answer {
                Ok(all) => {
                    let mine: Vec<CheckSummary> = all
                        .into_iter()
                        .filter(|c| {
                            stack.is_none_or(|s| c.check.stack_id == Some(s.get_untracked()))
                        })
                        .collect();
                    note.set(mine.is_empty().then(|| {
                        "No checks yet. A check says when something stops answering.".to_owned()
                    }));
                    list.set(mine);
                }
                Err(e) => note.set(Some(format!("Could not read the checks. {}", e.message))),
            }
        });
    };
    // Again when the stack page it is on moves to another stack.
    Effect::new(move |_| {
        let _ = stack.map(|s| s.get());
        refresh();
    });
    if let Some(events) = use_events() {
        let reload = screen.coalesce(Duration::from_millis(400), refresh);
        events.on(move |event| {
            if matches!(event, ServerEvent::CheckChanged { .. }) {
                reload();
            }
        });
        events.on_reconnect(refresh);
    }

    let here = move || stack.map_or_else(|| "/host".to_owned(), |s| format!("/stacks/{}", s.get()));

    view! {
        <h2 class="group-heading">"Uptime"</h2>
        {move || note.get().map(|n| view! { <p class="entry-note">{n}</p> })}
        <ul class="rows rows-checks">
            {move || {
                let from = here();
                list.get().into_iter().map(|s| {
                    let values = s.recent.iter().map(|v| v.map(f64::from)).collect();
                    view! {
                        <Row
                            state=state_key(s.status.state)
                            name=s.check.name.clone()
                            href=format!("/checks/{}?from={from}", s.check.id)
                            detail=detail(&s)
                        >
                            <Sparkline values />
                        </Row>
                    }
                }).collect_view()
            }}
        </ul>
        <a class="button button-quiet" href=move || match stack {
            Some(s) => format!("/checks/new?stack={}", s.get()),
            None => "/checks/new".to_owned(),
        }>"Add a check"</a>
    }
}

/// Adding a check.
#[component]
pub fn NewCheck() -> impl IntoView {
    let navigate = use_navigate();
    let query = use_query_map();
    let stack = query.with_untracked(|q| q.get("stack").and_then(|s| s.parse::<i64>().ok()));
    let back = came_from("/host");
    view! {
        <Topbar title="New check" back />
        <CheckForm
            current=None
            stack
            on_saved=Callback::new(move |made: CheckSummary| {
                navigate(&format!("/checks/{}", made.check.id), Default::default());
            })
        />
    }
}

/// One check: how it is doing, its latency and uptime over time, when it
/// was down, and its settings.
#[component]
pub fn CheckDetail() -> impl IntoView {
    let id = route_id();
    let back = came_from("/host");
    let navigate = use_navigate();
    let screen = Screen::new();
    let summary = RwSignal::new(None::<CheckSummary>);
    let range = RwSignal::new(Range::Day);
    let points = RwSignal::new(Vec::<Reading>::new());
    let step = RwSignal::new(60_i64);
    let uptime = RwSignal::new(None::<f64>);
    let incidents = RwSignal::new(Vec::<Incident>::new());
    let error = RwSignal::new(None::<String>);
    let editing = RwSignal::new(false);
    let removing = RwSignal::new(false);
    // Switching range quickly leaves reads in flight; only the newest lands.
    let latest = StoredValue::new(0_u64);

    let read = move || {
        let id = id.get_untracked();
        screen.load(async move {
            match api::check(id).await {
                Ok(s) => summary.set(Some(s)),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let read_history = move || {
        let (id, r) = (id.get_untracked(), range.get_untracked());
        let mine = latest.get_value() + 1;
        latest.set_value(mine);
        screen.load(async move {
            let answer = api::check_history(id, r).await;
            if latest.try_get_value() != Some(mine) {
                return;
            }
            match answer {
                Ok(h) => {
                    step.set(h.step);
                    points.set(h.points.iter().map(reading).collect());
                    uptime.set(h.uptime);
                    incidents.set(h.incidents);
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    Effect::new(move |_| {
        let _ = id.get();
        read();
    });
    Effect::new(move |_| {
        let _ = (id.get(), range.get());
        read_history();
    });
    if let Some(events) = use_events() {
        let reload = screen.coalesce(Duration::from_millis(400), move || {
            read();
            read_history();
        });
        events.on(move |event| {
            if let ServerEvent::CheckChanged { check } = event
                && check.check_id == id.get_untracked()
            {
                reload();
            }
        });
        events.on_reconnect(move || {
            read();
            read_history();
        });
    }

    let remove = Callback::new(move |()| {
        if removing.get_untracked() {
            return;
        }
        removing.set(true);
        error.set(None);
        let navigate = navigate.clone();
        let to = back.get_untracked();
        screen.act(
            api::delete_check(id.get_untracked()),
            move |result| match result {
                Ok(()) => navigate(&to, Default::default()),
                Err(e) => {
                    error.set(Some(e.message));
                    removing.set(false);
                }
            },
        );
    });

    view! {
        <Topbar
            title=Signal::derive(move || {
                summary.with(|s| s.as_ref().map_or_else(|| "Check".to_owned(), |s| s.check.name.clone()))
            })
            back
        />
        <ErrorNotice error />
        {move || summary.get().map(|s| view! {
            <ul class="rows">
                <Row state=state_key(s.status.state) name=s.check.target.clone() ident=true detail=detail(&s) />
            </ul>
            <p class="entry-note">{format!(
                "Runs every {} s from GhostDock's own network; failures in a row before it is down: {}.{}",
                s.check.interval_s,
                s.check.retries,
                if s.check.notify { " Its changes are sent to the alert channels." } else { "" },
            )}</p>
        })}

        <RangePicker range />
        <p class="verdict-count">{move || format!("{} up over this range", format_uptime(uptime.get()))}</p>
        <div class="charts">
            <Chart points=points.into() measure=Measure::Latency range=range.into() step=step.into() />
            <Chart points=points.into() measure=Measure::Uptime range=range.into() step=step.into() />
        </div>

        <h2 class="group-heading">"Incidents"</h2>
        {move || {
            let list = incidents.get();
            if list.is_empty() {
                return view! { <p class="entry-note">"None in this range."</p> }.into_any();
            }
            let now = chrono::Utc::now().timestamp();
            view! {
                <ul class="rows">
                    {list.into_iter().map(|i| {
                        let (state, words) = match i.ended_at {
                            None => ("unhealthy", format!("down now, for {}", lasted(now - i.started_at.timestamp()))),
                            Some(end) => ("none", format!("down for {}", lasted((end - i.started_at).num_seconds()))),
                        };
                        view! {
                            <Row
                                state
                                name=crate::time::local(i.started_at, "%d %b %H:%M")
                                detail=format!("{words}: {}", i.cause)
                            />
                        }
                    }).collect_view()}
                </ul>
            }
            .into_any()
        }}

        <h2 class="group-heading">"Settings"</h2>
        {move || match (editing.get(), summary.with(|s| s.as_ref().map(|s| s.check.clone()))) {
            (true, Some(check)) => view! {
                <CheckForm
                    current=Some(check)
                    stack=None
                    on_saved=Callback::new(move |saved: CheckSummary| {
                        summary.set(Some(saved));
                        editing.set(false);
                    })
                />
            }
            .into_any(),
            _ => view! {
                <button class="button button-quiet" type="button" on:click=move |_| editing.set(true)>
                    "Change settings"
                </button>
            }
            .into_any(),
        }}
        <Confirm
            label="Remove this check"
            confirm="Remove it"
            disabled=Signal::derive(move || removing.get())
            on_confirm=remove
        />
        <p class="entry-note">"Removes it with its history and incidents."</p>
    }
}

/// A number typed into a field, or the reason it is not one.
fn number<T: std::str::FromStr>(text: &str, what: &str) -> Result<T, String> {
    text.trim()
        .parse()
        .map_err(|_| format!("{what} must be a whole number."))
}

/// The settings a form's fields hold, all of them, so a change leaves
/// nothing behind that was cleared.
#[allow(clippy::too_many_arguments)]
fn input(
    name: &str,
    kind: CheckKind,
    target: &str,
    interval: &str,
    timeout: &str,
    retries: &str,
    statuses: (&str, &str),
    keyword: &str,
    slow: &str,
    stack: &str,
    flags: (bool, bool),
) -> Result<CheckInput, String> {
    let slow = slow.trim();
    Ok(CheckInput {
        name: Some(name.to_owned()),
        kind: Some(kind),
        target: Some(target.trim().to_owned()),
        interval_s: Some(number(interval, "The interval")?),
        timeout_s: Some(number(timeout, "The timeout")?),
        retries: Some(number(retries, "Failures before down")?),
        expect_status_min: Some(number(statuses.0, "The lowest status")?),
        expect_status_max: Some(number(statuses.1, "The highest status")?),
        keyword: Some(keyword.to_owned()),
        latency_warn_ms: Some(if slow.is_empty() {
            0
        } else {
            number(slow, "The latency limit")?
        }),
        stack_id: Some(stack.parse().unwrap_or(0)),
        notify: Some(flags.0),
        enabled: Some(flags.1),
    })
}

/// The fields of a check. With `current` it changes that one.
#[component]
fn CheckForm(
    current: Option<Check>,
    stack: Option<i64>,
    on_saved: Callback<CheckSummary>,
) -> impl IntoView {
    let screen = Screen::new();
    let id = current.as_ref().map(|c| c.id);
    let text = |f: fn(&Check) -> String, default: &str| {
        RwSignal::new(current.as_ref().map_or_else(|| default.to_owned(), f))
    };
    let name = text(|c| c.name.clone(), "");
    let target = text(|c| c.target.clone(), "");
    let interval = text(|c| c.interval_s.to_string(), "60");
    let timeout = text(|c| c.timeout_s.to_string(), "10");
    let retries = text(|c| c.retries.to_string(), "2");
    let low = text(|c| c.expect_status_min.to_string(), "200");
    let high = text(|c| c.expect_status_max.to_string(), "399");
    let keyword = text(|c| c.keyword.clone().unwrap_or_default(), "");
    let slow = text(
        |c| {
            c.latency_warn_ms
                .map(|ms| ms.to_string())
                .unwrap_or_default()
        },
        "",
    );
    let chosen_stack = RwSignal::new(
        current
            .as_ref()
            .and_then(|c| c.stack_id)
            .or(stack)
            .map(|s| s.to_string())
            .unwrap_or_default(),
    );
    let kind = text(|c| c.kind.as_str().to_owned(), "http");
    let notify = RwSignal::new(current.as_ref().is_none_or(|c| c.notify));
    let enabled = RwSignal::new(current.as_ref().is_none_or(|c| c.enabled));
    let stacks = RwSignal::new(options(&[("", "None")]));
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    screen.load(async move {
        if let Ok(list) = api::updates().await {
            stacks.update(|all| {
                all.extend(
                    list.into_iter()
                        .map(|u| (u.stack.id.to_string(), u.stack.name)),
                );
            });
        }
    });

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let made = input(
            &name.get_untracked(),
            CheckKind::parse(&kind.get_untracked()).unwrap_or(CheckKind::Http),
            &target.get_untracked(),
            &interval.get_untracked(),
            &timeout.get_untracked(),
            &retries.get_untracked(),
            (&low.get_untracked(), &high.get_untracked()),
            &keyword.get_untracked(),
            &slow.get_untracked(),
            &chosen_stack.get_untracked(),
            (notify.get_untracked(), enabled.get_untracked()),
        );
        let made = match made {
            Ok(made) => made,
            Err(message) => {
                error.set(Some(message));
                return;
            }
        };
        busy.set(true);
        error.set(None);
        screen.act(
            async move { api::save_check(id, &made).await },
            move |result| {
                match result {
                    Ok(saved) => on_saved.run(saved),
                    Err(e) => error.set(Some(e.message)),
                }
                busy.set(false);
            },
        );
    };

    view! {
        <form on:submit=submit>
            <ErrorNotice error />
            <TextField label="Name" value=name placeholder="Blog" />
            <Pick label="Kind" value=kind options=options(&[
                ("http", "HTTP(S): a URL answers"),
                ("tcp", "TCP: a port accepts connections"),
                ("container", "Container: running and not unhealthy"),
            ]) />
            <TextField label="Target: a URL, host:port or container name" value=target
                placeholder="https://blog.example.com/health" />
            <p class="entry-note">
                "Checks run from GhostDock's own network. A service reachable only inside a \
                 stack's network needs a container check or a published port."
            </p>
            <TextField label="Every (seconds)" value=interval />
            <TextField label="Timeout (seconds)" value=timeout />
            <TextField label="Down after this many failures in a row" value=retries />
            <TextField label="Degraded when slower than (ms, blank for never)" value=slow />
            <Show when=move || kind.with(|k| k == "http")>
                <TextField label="Lowest accepted status" value=low />
                <TextField label="Highest accepted status" value=high />
                <TextField label="Page must contain (optional)" value=keyword />
            </Show>
            <Pick label="Stack" value=chosen_stack options=stacks />
            <Tick name="Alerts" detail="Send its changes to every alert channel" value=notify />
            <Tick name="Running" detail="Turned off, it is paused and keeps its history" value=enabled />
            <button class="button" type="submit" disabled=move || busy.get()>
                {move || if busy.get() { "Saving" } else if id.is_some() { "Save changes" } else { "Add check" }}
            </button>
        </form>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_is_drawn_as_one_the_stylesheet_knows() {
        assert_eq!(state_key(CheckState::Up), "running");
        assert_eq!(state_key(CheckState::Down), "unhealthy");
        assert_eq!(state_key(CheckState::Degraded), "degraded");
        assert_eq!(state_key(CheckState::Pending), "busy");
        assert_eq!(state_key(CheckState::Paused), "stopped");
    }

    #[test]
    fn a_duration_is_said_in_the_largest_unit_that_fits() {
        assert_eq!(lasted(20), "under a minute");
        assert_eq!(lasted(125), "2 min");
        assert_eq!(lasted(3_720), "1 h 2 min");
        assert_eq!(lasted(3 * 86_400), "3 days");
    }

    #[test]
    fn a_point_carries_latency_and_uptime_where_the_chart_reads_them() {
        let r = reading(&CheckPoint {
            t: 60,
            up: 3,
            total: 4,
            latency_avg: Some(120.0),
            latency_max: Some(300.0),
        });
        assert_eq!(
            Measure::Latency.values(&r),
            (Some(120.0), Some(300.0), None)
        );
        assert_eq!(Measure::Uptime.values(&r), (Some(75.0), None, None));
        let empty = reading(&CheckPoint {
            t: 0,
            up: 0,
            total: 0,
            latency_avg: None,
            latency_max: None,
        });
        assert_eq!(
            Measure::Uptime.values(&empty).0,
            None,
            "no runs is a gap, not 0%"
        );
    }

    #[test]
    fn a_form_sends_every_setting_and_clears_what_is_blank() {
        let made = input(
            "Blog",
            CheckKind::Http,
            " https://a.test/ ",
            "60",
            "10",
            "2",
            ("200", "299"),
            "",
            "",
            "",
            (true, false),
        )
        .unwrap();
        assert_eq!(made.target.as_deref(), Some("https://a.test/"));
        assert_eq!(made.keyword.as_deref(), Some(""), "empty clears a keyword");
        assert_eq!(made.latency_warn_ms, Some(0), "blank clears the limit");
        assert_eq!(made.stack_id, Some(0), "no stack unlinks one");
        assert_eq!((made.notify, made.enabled), (Some(true), Some(false)));
        let bad = input(
            "Blog",
            CheckKind::Http,
            "https://a.test/",
            "soon",
            "10",
            "2",
            ("200", "299"),
            "",
            "",
            "",
            (true, true),
        );
        assert!(bad.unwrap_err().contains("interval"));
    }
}
