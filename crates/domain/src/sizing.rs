//! Limits a container's history supports, with the evidence.
//!
//! Two steps: a [`Summary`] of the history (peaks, percentiles, the latest
//! limit), then [`advise`] on that summary. The store computes the same
//! summary in SQL, so a month of minutes never has to be read row by row;
//! [`summarise`] is the definition it is tested against.

use shared::metrics::{Flag, Reading, Recommendation, Severity, format_bytes};

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;
const MINUTES_PER_DAY: f64 = 1440.0;
const MIN_DAYS: f64 = 3.0;

/// A container's one-minute rows and what happened to it.
#[derive(Debug, Clone, Copy)]
pub struct History<'a> {
    pub container: &'a str,
    pub project: Option<&'a str>,
    pub service: Option<&'a str>,
    /// One-minute rows, oldest first; only minutes it was running.
    pub minutes: &'a [Reading],
    pub ooms: u32,
}

/// What the rules need from a history.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Summary {
    /// Minutes with a figure: the minutes it was running.
    pub running: u64,
    /// Highest memory seen, peaks included.
    pub mem_peak: u64,
    /// Median of the per-minute memory averages.
    pub mem_typical: u64,
    /// The limit in force at the last running minute.
    pub mem_limit: Option<u64>,
    /// Median and 95th percentile of the per-minute CPU averages, in cores.
    pub cpu_typical: f64,
    pub cpu_p95: f64,
    /// Highest CPU seen, peaks included.
    pub cpu_burst: f64,
    /// Mean share of CPU periods throttled, where measured.
    pub throttled: Option<f64>,
}

/// The index of the `p` quantile in `n` sorted values: the nearest rank.
/// Shared with the store's SQL, so both pick the same value.
#[must_use]
pub fn rank(n: u64, p: f64) -> u64 {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let idx = ((n.saturating_sub(1) as f64) * p).round() as u64;
    idx
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    let idx = usize::try_from(rank(values.len() as u64, p)).unwrap_or(usize::MAX);
    values.get(idx).copied().unwrap_or(0.0)
}

#[must_use]
pub fn summarise(minutes: &[Reading]) -> Summary {
    let running: Vec<&Reading> = minutes
        .iter()
        .filter(|r| r.cpu.is_some() || r.mem.is_some())
        .collect();
    let mut cpu: Vec<f64> = running.iter().filter_map(|r| r.cpu).collect();
    #[allow(clippy::cast_precision_loss)]
    let mut mem: Vec<f64> = running
        .iter()
        .filter_map(|r| r.mem)
        .map(|m| m as f64)
        .collect();
    let throttle: Vec<f64> = running.iter().filter_map(|r| r.throttled).collect();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mem_typical = percentile(&mut mem, 0.5) as u64;
    #[allow(clippy::cast_precision_loss)]
    let throttled =
        (!throttle.is_empty()).then(|| throttle.iter().sum::<f64>() / throttle.len() as f64);
    Summary {
        running: running.len() as u64,
        mem_peak: running
            .iter()
            .filter_map(|r| r.mem_max.or(r.mem))
            .max()
            .unwrap_or(0),
        mem_typical,
        mem_limit: running
            .last()
            .map_or_else(|| minutes.last().and_then(|r| r.mem_limit), |r| r.mem_limit),
        cpu_typical: percentile(&mut cpu.clone(), 0.5),
        cpu_p95: percentile(&mut cpu, 0.95),
        cpu_burst: running
            .iter()
            .filter_map(|r| r.cpu_max.or(r.cpu))
            .fold(0.0, f64::max),
        throttled,
    }
}

fn round_memory(bytes: f64) -> u64 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let b = bytes.max(0.0).ceil() as u64;
    let step = if b <= GIB { 64 * MIB } else { 256 * MIB };
    b.div_ceil(step).max(1) * step
}

/// Up to the next 0.05 of a core. The small allowance keeps floating-point
/// error (0.2 × 1.5 is 0.30000000000000004) from rounding 0.30 up to 0.35.
fn round_cpus(cores: f64) -> f64 {
    (((cores / 0.05) - 1e-9).ceil() * 0.05).max(0.1)
}

#[must_use]
pub fn recommend(h: &History) -> Recommendation {
    advise(
        h.container,
        h.project,
        h.service,
        &summarise(h.minutes),
        h.ooms,
    )
}

#[must_use]
#[allow(clippy::cast_precision_loss)] // Byte counts as display figures.
pub fn advise(
    container: &str,
    project: Option<&str>,
    service: Option<&str>,
    s: &Summary,
    ooms: u32,
) -> Recommendation {
    let days = s.running as f64 / MINUTES_PER_DAY;
    let (peak, limit) = (s.mem_peak, s.mem_limit);

    let mut flags = Vec::new();
    if ooms > 0 {
        flags.push(Flag {
            severity: Severity::Bad,
            text: if ooms == 1 {
                "OOM-killed once in the last 30 days".to_owned()
            } else {
                format!("OOM-killed {ooms} times in the last 30 days")
            },
        });
    }
    if let Some(l) = limit
        && l > 0
        && peak as f64 > 0.9 * l as f64
    {
        flags.push(Flag {
            severity: Severity::Degraded,
            text: format!(
                "memory peaked at {}, {:.0}% of its limit",
                format_bytes(peak),
                peak as f64 / l as f64 * 100.0
            ),
        });
    }
    if let Some(t) = s.throttled.filter(|t| *t > 0.10) {
        flags.push(Flag {
            severity: Severity::Degraded,
            text: format!("CPU-throttled {:.0}% of the time", t * 100.0),
        });
    }
    match limit {
        None => flags.push(Flag {
            severity: Severity::Info,
            text: "no memory limit: it can use all of the host's memory".to_owned(),
        }),
        Some(l) if peak > 0 && l > 4 * peak && l.saturating_sub(peak) >= 256 * MIB => {
            flags.push(Flag {
                severity: Severity::Info,
                text: format!(
                    "limited to {} but never used more than {}",
                    format_bytes(l),
                    format_bytes(peak)
                ),
            });
        }
        Some(_) => {}
    }

    let base = Recommendation {
        container: container.to_owned(),
        project: project.map(str::to_owned),
        service: service.map(str::to_owned),
        days,
        evidence: String::new(),
        cpus: None,
        memory: None,
        flags,
        snippet: None,
    };
    if days < MIN_DAYS {
        return Recommendation {
            evidence: format!("Not enough history yet: {days:.1} days of the 3 days needed."),
            ..base
        };
    }

    // After an OOM kill the peak was capped by the old limit, so it says
    // nothing about what the container wanted.
    let wanted = match (ooms, limit) {
        (n, Some(l)) if n > 0 => (peak as f64 * 1.3).max(l as f64 * 1.5),
        _ => peak as f64 * 1.3,
    };
    let memory = round_memory(wanted).max(64 * MIB);
    let cpus = round_cpus(s.cpu_p95 * 1.5);

    let mut evidence = format!(
        "Over {days:.0} days: memory peaked at {}, typically {}; CPU typically {:.2} cores, {:.2} at the 95th percentile, bursts to {:.2}.",
        format_bytes(peak),
        format_bytes(s.mem_typical),
        s.cpu_typical,
        s.cpu_p95,
        s.cpu_burst,
    );
    if s.cpu_burst > cpus {
        evidence.push_str(&format!(" A limit of {cpus:.2} would slow those bursts."));
    }
    let name = service.unwrap_or(container);
    let snippet = format!(
        "services:\n  {name}:\n    deploy:\n      resources:\n        limits:\n          cpus: \"{cpus:.2}\"\n          memory: {}M\n",
        memory / MIB
    );
    Recommendation {
        evidence,
        cpus: Some(cpus),
        memory: Some(memory),
        snippet: Some(snippet),
        ..base
    }
}
