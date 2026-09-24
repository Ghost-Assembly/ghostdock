//! Charts, drawn as SVG: small, crisp at any size, and no script.
//!
//! Series arrive thinned to at most 300 points, so the cost of a chart does
//! not depend on its range. The average is a filled area and the peak a faint
//! line above it: a spike stays visible however much time a point stands
//! for. Where nothing was measured, the line breaks rather than falling to
//! zero, because a stopped container is not an idle one.

use leptos::prelude::*;
use shared::metrics::{Range, Reading, format_bytes, format_cores, format_rate};

const W: f64 = 600.0;
const H: f64 = 160.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measure {
    Cpu,
    Memory,
    NetIn,
    NetOut,
    DiskRead,
    DiskWrite,
    Load,
}

impl Measure {
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "Memory",
            Self::NetIn => "Network in",
            Self::NetOut => "Network out",
            Self::DiskRead => "Disk reads",
            Self::DiskWrite => "Disk writes",
            Self::Load => "Load",
        }
    }

    /// Average, peak and limit at one point.
    #[allow(clippy::cast_precision_loss)]
    #[must_use]
    pub fn values(self, r: &Reading) -> (Option<f64>, Option<f64>, Option<f64>) {
        let b = |v: Option<u64>| v.map(|v| v as f64);
        match self {
            Self::Cpu => (r.cpu, r.cpu_max, None),
            Self::Memory => (b(r.mem), b(r.mem_max), b(r.mem_limit)),
            Self::NetIn => (r.net_rx, None, None),
            Self::NetOut => (r.net_tx, None, None),
            Self::DiskRead => (r.io_read, None, None),
            Self::DiskWrite => (r.io_write, None, None),
            Self::Load => (r.load, None, None),
        }
    }

    /// The top of a scale holding `v`: round in the units the figure is
    /// shown in, so memory reads "100 GiB" rather than "93 GiB".
    #[must_use]
    pub fn ceiling(self, v: f64) -> f64 {
        match self {
            Self::Cpu | Self::Load => nice_ceiling(v),
            _ => {
                let mut unit = 1.0;
                while v.is_finite() && v / unit >= 1024.0 {
                    unit *= 1024.0;
                }
                // 1000 of a unit is shown as 1000; the next unit's 1 reads better.
                let top = nice_ceiling(v / unit);
                if top >= 1000.0 {
                    1024.0 * unit
                } else {
                    top * unit
                }
            }
        }
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    #[must_use]
    pub fn format(self, v: f64) -> String {
        match self {
            Self::Cpu => format_cores(v),
            Self::Memory => format_bytes(v.max(0.0).round() as u64),
            Self::Load => format!("{v:.2}"),
            _ => format_rate(v),
        }
    }
}

/// The next round number at or above `v`: 1, 2, 2.5 or 5 times a power of ten.
#[must_use]
pub fn nice_ceiling(v: f64) -> f64 {
    if v <= 0.0 || !v.is_finite() {
        return 1.0;
    }
    let power = 10_f64.powf(v.log10().floor());
    [1.0, 2.0, 2.5, 5.0, 10.0]
        .into_iter()
        .map(|m| m * power)
        .find(|c| *c >= v - v * 1e-12)
        .unwrap_or(10.0 * power)
}

fn xy(t: i64, v: f64, t0: i64, t1: i64, max: f64) -> (f64, f64) {
    #[allow(clippy::cast_precision_loss)]
    let x = if t1 > t0 {
        (t - t0) as f64 / (t1 - t0) as f64 * W
    } else {
        0.0
    };
    let y = H - (v / max).clamp(0.0, 1.0) * H;
    (x, y)
}

/// Runs of consecutive measured points: a `None`, or a jump in time longer
/// than `gap` seconds, ends a run.
fn runs(points: &[(i64, Option<f64>)], gap: i64) -> Vec<Vec<(i64, f64)>> {
    let mut all = Vec::new();
    let mut run: Vec<(i64, f64)> = Vec::new();
    let mut last_t: Option<i64> = None;
    for (t, v) in points {
        let broken = last_t.is_some_and(|l| t - l > gap);
        match v {
            Some(v) if !broken => run.push((*t, *v)),
            Some(v) => {
                if !run.is_empty() {
                    all.push(std::mem::take(&mut run));
                }
                run.push((*t, *v));
            }
            None => {
                if !run.is_empty() {
                    all.push(std::mem::take(&mut run));
                }
            }
        }
        last_t = Some(*t);
    }
    if !run.is_empty() {
        all.push(run);
    }
    all
}

#[must_use]
pub fn path(points: &[(i64, Option<f64>)], t0: i64, t1: i64, max: f64, gap: i64) -> String {
    runs(points, gap)
        .iter()
        .map(|run| {
            run.iter()
                .enumerate()
                .map(|(i, (t, v))| {
                    let (x, y) = xy(*t, *v, t0, t1, max);
                    format!("{}{x:.1} {y:.1}", if i == 0 { "M" } else { "L" })
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[must_use]
pub fn area(points: &[(i64, Option<f64>)], t0: i64, t1: i64, max: f64, gap: i64) -> String {
    runs(points, gap)
        .iter()
        .filter_map(|run| {
            let (first, _) = run.first()?;
            let (last, _) = run.last()?;
            let (x0, _) = xy(*first, 0.0, t0, t1, max);
            let (x1, _) = xy(*last, 0.0, t0, t1, max);
            let line: Vec<String> = run
                .iter()
                .map(|(t, v)| {
                    let (x, y) = xy(*t, *v, t0, t1, max);
                    format!("L{x:.1} {y:.1}")
                })
                .collect();
            Some(format!(
                "M{x0:.1} {H:.1} {} L{x1:.1} {H:.1} Z",
                line.join(" ")
            ))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The seconds a chart shows: `range` back from now, so a container that
/// stopped an hour ago shows an hour of nothing at the right, or back from
/// the newest point if the server's clock runs ahead of this one.
fn span(points: &[Reading], range: Range, now: i64) -> (i64, i64) {
    let t1 = points.last().map_or(now, |r| r.t.max(now));
    (t1 - range.seconds(), t1)
}

fn time_label(t: i64, range: Range) -> String {
    let Some(at) = chrono::DateTime::from_timestamp(t, 0) else {
        return String::new();
    };
    let local = at.with_timezone(&chrono::Local);
    match range {
        Range::Hour | Range::Day => local.format("%H:%M").to_string(),
        Range::Week | Range::Month => local.format("%d %b").to_string(),
        Range::Year => local.format("%b %Y").to_string(),
    }
}

/// One measure over a range. Tap or hover to read a moment's value.
#[component]
pub fn Chart(
    points: Signal<Vec<Reading>>,
    measure: Measure,
    range: Signal<Range>,
    step: Signal<i64>,
) -> impl IntoView {
    let picked = RwSignal::new(None::<usize>);
    let geometry = Memo::new(move |_| {
        let pts = points.get();
        let (t0, t1) = span(&pts, range.get(), chrono::Utc::now().timestamp());
        let avg: Vec<(i64, Option<f64>)> = pts.iter().map(|r| (r.t, measure.values(r).0)).collect();
        let peak: Vec<(i64, Option<f64>)> =
            pts.iter().map(|r| (r.t, measure.values(r).1)).collect();
        let limit = pts.iter().rev().find_map(|r| measure.values(r).2);
        let top = pts
            .iter()
            .filter_map(|r| {
                let (a, p, _) = measure.values(r);
                p.or(a)
            })
            .fold(limit.unwrap_or(0.0), f64::max);
        let max = measure.ceiling(top);
        // Points further apart than two steps (plus thinning) are a gap.
        let gap = (step.get() * 2).max((t1 - t0) / 150);
        let latest = pts.iter().rev().find_map(|r| measure.values(r).0);
        let near_limit = matches!((latest, limit), (Some(v), Some(l)) if l > 0.0 && v > 0.9 * l);
        (
            area(&avg, t0, t1, max, gap),
            path(&peak, t0, t1, max, gap),
            limit.map(|l| H - (l / max).clamp(0.0, 1.0) * H),
            max,
            t0,
            t1,
            latest,
            near_limit,
        )
    });

    let readout = move || {
        let pts = points.get();
        let at = picked.get().and_then(|i| pts.get(i)).or_else(|| pts.last());
        match at {
            Some(r) => {
                let (avg, peak, _) = measure.values(r);
                let value = avg.map_or_else(|| "nothing running".to_owned(), |v| measure.format(v));
                let peak = peak
                    .filter(|p| avg.is_some_and(|a| *p > a * 1.05))
                    .map(|p| format!(", peak {}", measure.format(p)))
                    .unwrap_or_default();
                format!("{value}{peak}")
            }
            None => "no figures yet".to_owned(),
        }
    };

    let on_move = move |ev: leptos::ev::PointerEvent| {
        let Some(target) = ev.current_target() else {
            return;
        };
        let Ok(el) = wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(target) else {
            return;
        };
        let rect = el.get_bounding_client_rect();
        if rect.width() <= 0.0 {
            return;
        }
        let frac = ((f64::from(ev.client_x()) - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let (.., t0, t1, _, _) = geometry.get_untracked();
        #[allow(clippy::cast_possible_truncation)]
        let t = t0 + ((t1 - t0) as f64 * frac) as i64;
        let pts = points.get_untracked();
        let nearest = pts
            .iter()
            .enumerate()
            .min_by_key(|(_, r)| (r.t - t).abs())
            .map(|(i, _)| i);
        picked.set(nearest);
    };

    view! {
        <figure class="chart" data-state=move || if geometry.get().7 { "degraded" } else { "" }>
            <figcaption class="chart-head">
                <span class="chart-title">{measure.title()}</span>
                <span class="chart-value">{readout}</span>
            </figcaption>
            <div class="chart-plot">
                <svg viewBox="0 0 600 160" preserveAspectRatio="none" role="img"
                    aria-label=measure.title()
                    on:pointermove=on_move
                    on:pointerleave=move |_| picked.set(None)>
                    <path class="chart-area" d=move || geometry.get().0 />
                    <path class="chart-peak" d=move || geometry.get().1 />
                    {move || geometry.get().2.map(|y| view! {
                        <line class="chart-limit" x1="0" x2="600" y1=y y2=y />
                    })}
                </svg>
                // With nothing measured, a scale and times would only mislead.
                <Show when=move || points.with(|p| !p.is_empty())>
                    <span class="chart-axis chart-axis-top">{move || measure.format(geometry.get().3)}</span>
                    <span class="chart-axis chart-axis-start">{move || time_label(geometry.get().4, range.get())}</span>
                    <span class="chart-axis chart-axis-end">{move || time_label(geometry.get().5, range.get())}</span>
                </Show>
            </div>
        </figure>
    }
}

/// A last-hour trend in a row, where a chart would be too much.
#[component]
pub fn Sparkline(values: Vec<Option<f64>>) -> impl IntoView {
    let top = nice_ceiling(values.iter().flatten().fold(0.0, |a: f64, b| a.max(*b)));
    #[allow(clippy::cast_possible_wrap)]
    let points: Vec<(i64, Option<f64>)> = values
        .iter()
        .enumerate()
        .map(|(i, v)| (i as i64, *v))
        .collect();
    #[allow(clippy::cast_possible_wrap)]
    let end = (values.len().max(2) - 1) as i64;
    let d = path(&points, 0, end, top, 1);
    view! {
        <svg class="sparkline" viewBox="0 0 600 160" preserveAspectRatio="none" aria-hidden="true">
            <path d=d />
        </svg>
    }
}

/// How full something is. Amber past 80 %, red past 90 %.
#[component]
pub fn UsageBar(used: u64, total: u64) -> impl IntoView {
    #[allow(clippy::cast_precision_loss)]
    let share = if total > 0 {
        used as f64 / total as f64
    } else {
        0.0
    };
    let state = if share > 0.9 {
        "unhealthy"
    } else if share > 0.8 {
        "degraded"
    } else {
        "running"
    };
    view! {
        <div class="usage" data-state=state role="meter" aria-valuenow=format!("{:.0}", share * 100.0) aria-valuemin="0" aria-valuemax="100">
            <div class="usage-fill" style=format!("width: {:.1}%", share * 100.0)></div>
        </div>
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn ceilings_are_round_numbers_a_person_reads() {
        assert!((nice_ceiling(0.37) - 0.5).abs() < 1e-9);
        assert!((nice_ceiling(3.2) - 5.0).abs() < 1e-9);
        assert!((nice_ceiling(412e6) - 500e6).abs() < 1.0);
        assert!(
            (nice_ceiling(0.0) - 1.0).abs() < 1e-9,
            "an idle chart still has a scale"
        );
    }

    #[test]
    fn byte_scales_are_round_in_the_units_they_are_shown_in() {
        const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
        assert!((Measure::Memory.ceiling(62.5 * GIB) - 100.0 * GIB).abs() < 1.0);
        assert!(
            (Measure::NetIn.ceiling(300.0 * 1024.0 * 1024.0) - 500.0 * 1024.0 * 1024.0).abs() < 1.0
        );
        assert!(
            (Measure::Memory.ceiling(540.0 * 1024.0) - 1024.0 * 1024.0).abs() < 1.0,
            "1 MiB, not 1000 KiB"
        );
        assert!(
            (Measure::Cpu.ceiling(3.2) - 5.0).abs() < 1e-9,
            "cores stay decimal"
        );
        assert!((Measure::Memory.ceiling(0.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_path_runs_left_to_right_top_is_the_maximum() {
        let d = path(
            &[(0, Some(0.0)), (50, Some(1.0)), (100, Some(0.5))],
            0,
            100,
            1.0,
            1_000,
        );
        assert_eq!(d, "M0.0 160.0 L300.0 0.0 L600.0 80.0");
    }

    #[test]
    fn a_path_breaks_where_nothing_was_measured() {
        let gap = path(
            &[(0, Some(1.0)), (10, None), (20, Some(1.0))],
            0,
            20,
            1.0,
            1_000,
        );
        assert_eq!(gap.matches('M').count(), 2, "{gap}");
        let jump = path(
            &[(0, Some(1.0)), (10, Some(1.0)), (500, Some(1.0))],
            0,
            500,
            1.0,
            60,
        );
        assert_eq!(
            jump.matches('M').count(),
            2,
            "a long silence is a gap: {jump}"
        );
    }

    #[test]
    fn a_chart_ends_now_even_when_the_figures_stopped_earlier() {
        let at = |t| Reading {
            t,
            ..Reading::default()
        };
        assert_eq!(span(&[at(1_000)], Range::Hour, 5_000), (1_400, 5_000));
        assert_eq!(
            span(&[], Range::Hour, 5_000),
            (1_400, 5_000),
            "an empty chart still spans the range"
        );
        assert_eq!(
            span(&[at(6_000)], Range::Hour, 5_000),
            (2_400, 6_000),
            "a server clock ahead of this one wins"
        );
    }

    #[test]
    fn an_area_closes_each_run_down_to_the_baseline() {
        let d = area(&[(0, Some(1.0)), (100, Some(1.0))], 0, 100, 1.0, 1_000);
        assert_eq!(d, "M0.0 160.0 L0.0 0.0 L600.0 0.0 L600.0 160.0 Z");
        assert_eq!(area(&[], 0, 1, 1.0, 1), "");
    }
}
