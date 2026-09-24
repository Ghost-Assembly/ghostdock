//! A container's cumulative counters, from the daemon's stats stream.

use bollard::models::ContainerStatsResponse;
use domain::metrics::Counters;

/// The counters in one stats message. `None` for the empty message the
/// daemon sends for a container that is not running.
#[must_use]
pub fn to_counters(s: &ContainerStatsResponse) -> Option<Counters> {
    let cpu = s.cpu_stats.as_ref()?;
    let cpu_ns = cpu.cpu_usage.as_ref()?.total_usage?;
    let memory = s.memory_stats.as_ref();
    // A stopped container still streams: zero CPU time, no memory figures.
    // A running one has always used some CPU.
    if cpu_ns == 0 && memory.and_then(|m| m.usage).is_none() {
        return None;
    }
    let usage = memory.and_then(|m| m.usage).unwrap_or(0);
    // As `docker stats` does: page cache the kernel can reclaim is not use.
    let cache = memory
        .and_then(|m| m.stats.as_ref())
        .and_then(|stats| {
            stats
                .get("inactive_file")
                .or_else(|| stats.get("total_inactive_file"))
                .copied()
        })
        .unwrap_or(0);
    let (net_rx, net_tx) = s
        .networks
        .iter()
        .flat_map(|n| n.values())
        .fold((0, 0), |(rx, tx), n| {
            (rx + n.rx_bytes.unwrap_or(0), tx + n.tx_bytes.unwrap_or(0))
        });
    let (io_read, io_write) = s
        .blkio_stats
        .as_ref()
        .and_then(|b| b.io_service_bytes_recursive.as_ref())
        .into_iter()
        .flatten()
        .fold((0, 0), |(r, w), e| {
            match e.op.as_deref().map(str::to_ascii_lowercase).as_deref() {
                Some("read") => (r + e.value.unwrap_or(0), w),
                Some("write") => (r, w + e.value.unwrap_or(0)),
                _ => (r, w),
            }
        });
    let throttling = cpu.throttling_data.as_ref();
    Some(Counters {
        cpu_ns,
        mem_used: usage.saturating_sub(cache),
        mem_limit: memory.and_then(|m| m.limit).unwrap_or(0),
        net_rx,
        net_tx,
        io_read,
        io_write,
        periods: throttling.and_then(|t| t.periods).unwrap_or(0),
        throttled_periods: throttling.and_then(|t| t.throttled_periods).unwrap_or(0),
    })
}
