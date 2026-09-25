//! Resource charts for one container, and the pieces other screens reuse.

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use shared::event::ServerEvent;
use shared::metrics::{ContainerFigures, Now, Range, Reading, SubjectKind, Target};

use crate::api;
use crate::charts::{Chart, Measure};
use crate::events::use_events;
use crate::load::Load;
use crate::screen::Screen;
use crate::status::usage;
use crate::ui::{Choices, ErrorNotice, Row, Topbar, came_from};

#[component]
pub fn RangePicker(range: RwSignal<Range>) -> impl IntoView {
    view! {
        <Choices
            class="choices range-picker"
            label="Range"
            options=Range::ALL.iter().map(|r| (r.as_str(), None)).collect()
            chosen=Signal::derive(move || {
                Range::ALL.iter().position(|r| *r == range.get()).unwrap_or_default()
            })
            on_choose=Callback::new(move |i: usize| {
                if let Some(r) = Range::ALL.get(i) {
                    range.set(*r);
                }
            })
        />
    }
}

/// Charts for one target over the chosen range. The hour range also takes
/// the live 5 s points as they come; the others are history and stay still.
#[component]
pub fn Charts(
    target: Signal<Target>,
    range: RwSignal<Range>,
    measures: &'static [Measure],
) -> impl IntoView {
    let screen = Screen::new();
    let points = RwSignal::new(Vec::<Reading>::new());
    let step = RwSignal::new(5_i64);
    let error = RwSignal::new(None::<String>);
    // Switching range quickly leaves earlier requests in flight; only the
    // newest may land.
    let latest = StoredValue::new(0_u64);

    Effect::new(move |_| {
        let (t, r) = (target.get(), range.get());
        let mine = latest.get_value() + 1;
        latest.set_value(mine);
        screen.load(async move {
            let answer = api::metrics_series(&t, r).await;
            if latest.try_get_value() != Some(mine) {
                return;
            }
            match answer {
                Ok(series) => {
                    step.set(series.step);
                    points.set(series.points);
                    error.set(None);
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    });

    if let Some(events) = use_events() {
        events.watch_metrics();
        events.on(move |event| {
            let ServerEvent::Metrics { now } = event else {
                return;
            };
            if range.get_untracked() != Range::Hour {
                return;
            }
            let latest = target.with_untracked(|target| match target {
                Target::Subject(SubjectKind::Host, _) => now.host,
                Target::Subject(SubjectKind::Container, key) => now
                    .containers
                    .iter()
                    .find(|c| c.key == *key)
                    .map(|c| c.reading),
                Target::Subject(SubjectKind::Disk, key) => {
                    now.disks.iter().find(|c| c.key == *key).map(|c| c.reading)
                }
                Target::Subject(SubjectKind::Network, key) => now
                    .networks
                    .iter()
                    .find(|c| c.key == *key)
                    .map(|c| c.reading),
                Target::Stack(project) => now
                    .stacks
                    .iter()
                    .find(|s| s.project == *project)
                    .map(|s| s.reading),
            });
            if let Some(reading) = latest {
                points.update(|all| append_live(all, reading, Range::Hour.seconds()));
            }
        });
    }

    view! {
        <ErrorNotice error />
        <div class="charts">
            {measures.iter().map(|m| view! {
                <Chart points=points.into() measure=*m range=range.into() step=step.into() />
            }).collect_view()}
        </div>
    }
}

/// The bar colour a recommendation earns: its worst flag.
#[must_use]
pub fn sizing_state(r: &shared::metrics::Recommendation) -> &'static str {
    use shared::metrics::Severity;
    match r.flags.iter().map(|f| f.severity).min() {
        Some(Severity::Bad) => "unhealthy",
        Some(Severity::Degraded) => "degraded",
        _ => "running",
    }
}

#[component]
pub fn SizingRows(list: Vec<shared::metrics::Recommendation>) -> impl IntoView {
    view! {
        <ul class="rows rows-sizing">
            {list.into_iter().map(|r| {
                let detail = r.flags.first().map_or_else(|| r.evidence.clone(), |f| f.text.clone());
                view! {
                    <Row
                        state=sizing_state(&r)
                        href=format!("/containers/{}/resources?from=/host", r.container)
                        name=r.container
                        ident=true
                        detail
                    />
                }
            }).collect_view()}
        </ul>
    }
}

/// One container's advice: what was seen, what is wrong, what to paste.
#[component]
pub fn SizingAdvice(advice: shared::metrics::Recommendation) -> impl IntoView {
    use shared::metrics::Severity;
    view! {
        <h2 class="group-heading">"Sizing"</h2>
        {advice.flags.iter().map(|f| {
            let class = if f.severity == Severity::Info { "entry-note" } else { "notice" };
            view! { <p class=class>{f.text.clone()}</p> }
        }).collect_view()}
        <p class="entry-note">{advice.evidence.clone()}</p>
        {advice.snippet.clone().map(|snippet| view! {
            <pre class="snippet" tabindex="0">{snippet}</pre>
            <p class="entry-note">"Tap to select, then paste into the compose file."</p>
        })}
    }
}

/// Each of a stack's containers: what it uses now, and typically (the 95th
/// percentile) and at its peak over the chosen range.
#[component]
pub fn ContainerFigureRows(
    project: Signal<String>,
    range: RwSignal<Range>,
    /// The screen these rows are on, for the way back from one of them.
    from: Signal<String>,
) -> impl IntoView {
    let screen = Screen::new();
    let list = RwSignal::new(Load::<Vec<ContainerFigures>>::Loading);
    let now = RwSignal::new(None::<Now>);
    let latest = StoredValue::new(0_u64);

    Effect::new(move |_| {
        let (p, r) = (project.get(), range.get());
        let mine = latest.get_value() + 1;
        latest.set_value(mine);
        screen.load(async move {
            let answer = api::container_figures(&p, r).await;
            if latest.try_get_value() != Some(mine) {
                return;
            }
            list.set(Load::from(answer));
        });
    });
    screen.load(async move {
        if let Ok(n) = api::metrics_now().await {
            now.set(Some(n));
        }
    });
    if let Some(events) = use_events() {
        events.watch_metrics();
        events.on(move |event| {
            if let ServerEvent::Metrics { now: fresh } = event {
                now.set(Some((**fresh).clone()));
            }
        });
    }

    view! {
        {move || match list.get() {
            Load::Loading => view! { <p class="entry-note">"Reading figures"</p> }.into_any(),
            Load::Failed(message) => view! {
                <p class="entry-note">{format!("Could not read its containers' figures. {message}")}</p>
            }
            .into_any(),
            Load::Ready(figures) if figures.is_empty() => view! {
                <p class="entry-note">"No figures for its containers in this range yet."</p>
            }
            .into_any(),
            Load::Ready(figures) => view! {
            <ul class="rows rows-figures">
                {figures.into_iter().map(|f| {
                    let key = f.key.clone();
                    let current = Memo::new(move |_| {
                        now.with(|n| n.as_ref().and_then(|n| n.containers.iter().find(|c| c.key == key).map(|c| c.reading)))
                    });
                    let typical = format!("typical {}", pair(f.cpu_typical, f.mem_typical));
                    let peak = format!("peak {}", pair(f.cpu_peak, f.mem_peak));
                    let state = Signal::derive(move || {
                        if current.with(Option::is_some) { "running" } else { "stopped" }
                    });
                    let now_line = Signal::derive(move || current.get().map_or_else(
                        || "not running".to_owned(),
                        |r| format!("now {}", pair(r.cpu, r.mem)),
                    ));
                    let href = format!("/containers/{}/resources?from={}", f.key, from.get_untracked());
                    view! {
                        <Row state href name=f.key ident=true>
                            <span class="row-detail">{now_line}</span>
                            <span class="row-detail">{typical}</span>
                            <span class="row-detail">{peak}</span>
                        </Row>
                    }
                }).collect_view()}
            </ul>
            }
            .into_any(),
        }}
    }
}

/// "0.12 cores, 300 MiB", from whichever figures there are.
fn pair(cpu: Option<f64>, mem: Option<u64>) -> String {
    usage(cpu, mem).unwrap_or_else(|| "not measured".to_owned())
}

/// Adds a live point to a series, keeping `span` seconds: by time, not by
/// count, since the series arrives thinned and ticks come closer together.
fn append_live(points: &mut Vec<Reading>, reading: Reading, span: i64) {
    if points.last().is_some_and(|last| last.t >= reading.t) {
        return;
    }
    points.push(reading);
    let since = reading.t - span;
    let old = points.iter().take_while(|p| p.t < since).count();
    points.drain(..old);
}

#[component]
pub fn ContainerResources() -> impl IntoView {
    let screen = Screen::new();
    let params = use_params_map();
    let name = Memo::new(move |_| params.get().get("name").unwrap_or_default());
    let range = RwSignal::new(Range::Hour);
    let target = Signal::derive(move || Target::Subject(SubjectKind::Container, name.get()));
    let advice = RwSignal::new(None);
    let advice_error = RwSignal::new(None::<String>);
    // Again when the route moves to another container.
    Effect::new(move |_| {
        let wanted = name.get();
        advice.set(None);
        screen.load(async move {
            match api::sizing().await {
                Ok(list) => {
                    advice.set(list.into_iter().find(|r| r.container == wanted));
                    advice_error.set(None);
                }
                Err(e) => advice_error.set(Some(e.message)),
            }
        });
    });
    view! {
        <Topbar title=name back=came_from("/host") />
        <RangePicker range />
        <Charts target range measures=&[Measure::Cpu, Measure::Memory, Measure::NetIn, Measure::NetOut, Measure::DiskRead, Measure::DiskWrite] />
        {move || advice.get().map(|a| view! { <SizingAdvice advice=a /> })}
        {move || advice_error.get().map(|message| view! {
            <p class="entry-note">{format!("Could not read the sizing advice. {message}")}</p>
        })}
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn at(t: i64) -> Reading {
        Reading {
            t,
            cpu: Some(0.1),
            ..Reading::default()
        }
    }

    #[test]
    fn the_live_hour_stays_an_hour_however_long_it_is_watched() {
        // The hour arrives thinned (a point per 15 s); ticks then add a
        // point per 5 s. After an hour open, the chart must still span one.
        let mut points: Vec<Reading> = (0..240).map(|i| at(i * 15)).collect();
        for i in 1..=720 {
            append_live(&mut points, at(3_585 + i * 5), 3_600);
        }
        let span = points.last().unwrap().t - points.first().unwrap().t;
        assert!((3_595..=3_600).contains(&span), "spans {span} s");
        let len = points.len();
        let last = points.last().unwrap().t;
        append_live(&mut points, at(last), 3_600);
        assert_eq!(points.len(), len, "a repeated tick is not added");
    }
}
