//! From raw counters to readings, and from many readings to few.

use std::time::Duration;

use shared::metrics::{ContainerFigures, Reading};

/// A container's cumulative counters at one moment, as Docker reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counters {
    pub cpu_ns: u64,
    pub mem_used: u64,
    pub mem_limit: u64,
    pub net_rx: u64,
    pub net_tx: u64,
    pub io_read: u64,
    pub io_write: u64,
    pub periods: u64,
    pub throttled_periods: u64,
}

#[allow(clippy::cast_precision_loss)] // Rates are display and sizing figures.
fn per_second(delta: u64, secs: f64) -> f64 {
    delta as f64 / secs
}

/// Length of a rolled-up period, in seconds.
pub const QUARTER_SECS: i64 = 900;

/// Which minutes to fold into quarter-hour rows, if any are due.
///
/// `written_to` is the end of the minutes written so far and `rolled_to`
/// the quarter boundary rolled up to last time (`None` since starting).
/// Keyed on the boundary having moved rather than on the clock landing on
/// one, so a minute that runs late never skips a quarter. Returns the range
/// `[from, to)` to roll up.
#[must_use]
pub fn quarter_due(written_to: i64, rolled_to: Option<i64>) -> Option<(i64, i64)> {
    let boundary = written_to - written_to.rem_euclid(QUARTER_SECS);
    match rolled_to {
        Some(done) if done >= boundary => None,
        Some(done) => Some((done, boundary)),
        None => Some((boundary - QUARTER_SECS, boundary)),
    }
}

/// Readings closer together than this give no rate.
pub const MIN_ELAPSED: Duration = Duration::from_millis(500);

/// One reading from two sets of counters `elapsed` apart. `None` when any
/// cumulative counter went backwards: the container was restarted or
/// recreated, and the difference means nothing. A memory limit at or above
/// the host's memory is Docker's way of saying there is none.
#[must_use]
pub fn rate(
    prev: &Counters,
    next: &Counters,
    elapsed: Duration,
    host_memory: Option<u64>,
) -> Option<Reading> {
    // The daemon sends a reading a second. Two arriving closer than half
    // that were buffered while this process was starved: their counters
    // are a second apart, their arrival is not, and the rate would be
    // absurd.
    if elapsed < MIN_ELAPSED {
        return None;
    }
    let secs = elapsed.as_secs_f64();
    let cpu = next.cpu_ns.checked_sub(prev.cpu_ns)?;
    let rx = next.net_rx.checked_sub(prev.net_rx)?;
    let tx = next.net_tx.checked_sub(prev.net_tx)?;
    let read = next.io_read.checked_sub(prev.io_read)?;
    let write = next.io_write.checked_sub(prev.io_write)?;
    let periods = next.periods.checked_sub(prev.periods)?;
    let throttled = next.throttled_periods.checked_sub(prev.throttled_periods)?;

    let cores = per_second(cpu, secs) / 1e9;
    let limit = (next.mem_limit > 0 && host_memory.is_none_or(|h| next.mem_limit < h))
        .then_some(next.mem_limit);
    #[allow(clippy::cast_precision_loss)]
    let throttled = (periods > 0).then(|| throttled as f64 / periods as f64);
    Some(Reading {
        t: 0,
        cpu: Some(cores),
        cpu_max: Some(cores),
        mem: Some(next.mem_used),
        mem_max: Some(next.mem_used),
        mem_limit: limit,
        net_rx: Some(per_second(rx, secs)),
        net_tx: Some(per_second(tx, secs)),
        io_read: Some(per_second(read, secs)),
        io_write: Some(per_second(write, secs)),
        throttled,
        load: None,
    })
}

#[derive(Debug, Default, Clone, Copy)]
struct Mean {
    sum: f64,
    n: u32,
}

impl Mean {
    fn add(&mut self, v: Option<f64>) {
        if let Some(v) = v {
            self.sum += v;
            self.n += 1;
        }
    }
    fn get(self) -> Option<f64> {
        (self.n > 0).then(|| self.sum / f64::from(self.n))
    }
}

fn max_f(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// Folds readings into one: averages of averages, maxima of peaks, the last
/// limit seen.
#[derive(Debug, Default, Clone)]
pub struct Accumulator {
    n: u32,
    cpu: Mean,
    cpu_max: Option<f64>,
    mem: Mean,
    mem_max: Option<u64>,
    mem_limit: Option<u64>,
    net_rx: Mean,
    net_tx: Mean,
    io_read: Mean,
    io_write: Mean,
    throttled: Mean,
    load: Mean,
}

impl Accumulator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn add(&mut self, r: &Reading) {
        self.n += 1;
        self.cpu.add(r.cpu);
        self.cpu_max = max_f(self.cpu_max, r.cpu_max.or(r.cpu));
        self.mem.add(r.mem.map(|m| m as f64));
        self.mem_max = self.mem_max.max(r.mem_max.or(r.mem));
        if r.mem_limit.is_some() || r.mem.is_some() {
            self.mem_limit = r.mem_limit;
        }
        self.net_rx.add(r.net_rx);
        self.net_tx.add(r.net_tx);
        self.io_read.add(r.io_read);
        self.io_write.add(r.io_write);
        self.throttled.add(r.throttled);
        self.load.add(r.load);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// The folded reading, stamped `t`, and a fresh start. `None` if nothing
    /// was added: no reading, not a zero one.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn finish(&mut self, t: i64) -> Option<Reading> {
        if self.is_empty() {
            return None;
        }
        let done = std::mem::take(self);
        Some(Reading {
            t,
            cpu: done.cpu.get(),
            cpu_max: done.cpu_max,
            mem: done.mem.get().map(|m| m.round() as u64),
            mem_max: done.mem_max,
            mem_limit: done.mem_limit,
            net_rx: done.net_rx.get(),
            net_tx: done.net_tx.get(),
            io_read: done.io_read.get(),
            io_write: done.io_write.get(),
            throttled: done.throttled.get(),
            load: done.load.get(),
        })
    }
}

/// A container's typical (the 95th percentile of its averages) and peak CPU
/// and memory over `points`. The store works out the same in SQL for
/// stored ranges; this is the definition it is tested against.
#[must_use]
pub fn figures(key: &str, service: Option<&str>, points: &[Reading]) -> ContainerFigures {
    let p95 = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        let rank = crate::sizing::rank(values.len() as u64, 0.95);
        usize::try_from(rank)
            .ok()
            .and_then(|i| values.get(i).copied())
    };
    #[allow(clippy::cast_precision_loss)]
    let mem: Vec<f64> = points
        .iter()
        .filter_map(|r| r.mem)
        .map(|m| m as f64)
        .collect();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mem_typical = p95(mem).map(|m| m as u64);
    ContainerFigures {
        key: key.to_owned(),
        service: service.map(str::to_owned),
        cpu_typical: p95(points.iter().filter_map(|r| r.cpu).collect()),
        cpu_peak: points
            .iter()
            .filter_map(|r| r.cpu_max.or(r.cpu))
            .reduce(f64::max),
        mem_typical,
        mem_peak: points.iter().filter_map(|r| r.mem_max.or(r.mem)).max(),
    }
}

/// At most `max` points, each folding a run of neighbours, so peaks survive.
#[must_use]
pub fn thin(points: &[Reading], max: usize) -> Vec<Reading> {
    if points.len() <= max || max == 0 {
        return points.to_vec();
    }
    let per = points.len().div_ceil(max);
    points
        .chunks(per)
        .filter_map(|chunk| {
            let mut acc = Accumulator::new();
            chunk.iter().for_each(|r| acc.add(r));
            acc.finish(chunk.first().map_or(0, |r| r.t))
        })
        .collect()
}

fn sum_f(values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    values.fold(None, |acc, v| match (acc, v) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    })
}

fn sum_u(values: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    values.fold(None, |acc, v| match (acc, v) {
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
        (a, b) => a.or(b),
    })
}

/// Readings taken at the same moment, summed: a stack's containers. The
/// stack has a memory limit only if every container has one.
#[must_use]
pub fn sum_at(t: i64, readings: &[Reading]) -> Reading {
    let limit = if readings.iter().all(|r| r.mem_limit.is_some()) {
        sum_u(readings.iter().map(|r| r.mem_limit))
    } else {
        None
    };
    Reading {
        t,
        cpu: sum_f(readings.iter().map(|r| r.cpu)),
        cpu_max: sum_f(readings.iter().map(|r| r.cpu_max)),
        mem: sum_u(readings.iter().map(|r| r.mem)),
        mem_max: sum_u(readings.iter().map(|r| r.mem_max)),
        mem_limit: limit,
        net_rx: sum_f(readings.iter().map(|r| r.net_rx)),
        net_tx: sum_f(readings.iter().map(|r| r.net_tx)),
        io_read: sum_f(readings.iter().map(|r| r.io_read)),
        io_write: sum_f(readings.iter().map(|r| r.io_write)),
        throttled: None,
        load: None,
    }
}

/// The host's own counters, from `/proc`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HostCounters {
    pub cpu_busy: u64,
    pub cpu_total: u64,
    pub cpus: u32,
    pub mem_total: u64,
    pub mem_available: u64,
    pub load1: Option<f64>,
}

/// Busy and total CPU time, and the number of CPUs, from `/proc/stat`.
#[must_use]
pub fn parse_proc_stat(text: &str) -> Option<(u64, u64, u32)> {
    let first = text.lines().next()?;
    let mut fields = first.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let values: Vec<u64> = fields.take(8).filter_map(|f| f.parse().ok()).collect();
    if values.len() < 5 {
        return None;
    }
    let total: u64 = values.iter().sum();
    let idle = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);
    let cpus = text
        .lines()
        .filter(|l| {
            l.strip_prefix("cpu")
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        })
        .count();
    Some((
        total.saturating_sub(idle),
        total,
        u32::try_from(cpus).unwrap_or(0).max(1),
    ))
}

/// Total and available memory in bytes, from `/proc/meminfo`.
#[must_use]
pub fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let field = |name: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|kb| kb.parse::<u64>().ok())
            .map(|kb| kb * 1024)
    };
    Some((field("MemTotal:")?, field("MemAvailable:")?))
}

#[must_use]
pub fn parse_loadavg(text: &str) -> Option<f64> {
    text.split_whitespace().next()?.parse().ok()
}

/// Interfaces with their received and transmitted bytes, from
/// `/proc/<pid>/net/dev`. Loopback is left out.
#[must_use]
pub fn parse_net_dev(text: &str) -> Vec<(String, u64, u64)> {
    text.lines()
        .skip(2)
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let name = name.trim();
            // Loopback, and container plumbing: a veth per container and a
            // bridge per compose network, renamed on every deploy.
            if name == "lo" || name.starts_with("veth") || name.starts_with("br-") {
                return None;
            }
            let fields: Vec<u64> = rest
                .split_whitespace()
                .filter_map(|f| f.parse().ok())
                .collect();
            Some((name.to_owned(), *fields.first()?, *fields.get(8)?))
        })
        .collect()
}

/// The host between two readings of `/proc`: CPU as cores busy, memory as
/// total less available, so reclaimable cache does not count as used.
#[must_use]
pub fn host_rate(prev: &HostCounters, next: &HostCounters) -> Option<Reading> {
    let busy = next.cpu_busy.checked_sub(prev.cpu_busy)?;
    let total = next.cpu_total.checked_sub(prev.cpu_total)?;
    if total == 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let cores = busy as f64 / total as f64 * f64::from(next.cpus);
    let used = next.mem_total.saturating_sub(next.mem_available);
    Some(Reading {
        t: 0,
        cpu: Some(cores),
        cpu_max: Some(cores),
        mem: Some(used),
        mem_max: Some(used),
        mem_limit: Some(next.mem_total),
        load: next.load1,
        ..Reading::default()
    })
}

/// A network interface's rates between two readings.
#[must_use]
pub fn net_rate(prev: (u64, u64), next: (u64, u64), elapsed: Duration) -> Option<Reading> {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return None;
    }
    Some(Reading {
        net_rx: Some(per_second(next.0.checked_sub(prev.0)?, secs)),
        net_tx: Some(per_second(next.1.checked_sub(prev.1)?, secs)),
        ..Reading::default()
    })
}
