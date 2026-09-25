use std::time::Duration;

use domain::metrics::*;
use shared::metrics::Reading;

fn counters(cpu_ns: u64, net_rx: u64) -> Counters {
    Counters {
        cpu_ns,
        mem_used: 100 << 20,
        mem_limit: 512 << 20,
        net_rx,
        net_tx: 0,
        io_read: 0,
        io_write: 0,
        periods: 0,
        throttled_periods: 0,
    }
}

#[test]
fn cpu_is_measured_in_cores() {
    // Half a second of CPU in one second is half a core.
    let r = rate(
        &counters(0, 0),
        &counters(500_000_000, 0),
        Duration::from_secs(1),
        None,
    )
    .unwrap();
    assert!((r.cpu.unwrap() - 0.5).abs() < 1e-9);
    assert_eq!(r.cpu_max, r.cpu);
}

#[test]
fn byte_counters_become_rates() {
    let r = rate(
        &counters(0, 0),
        &counters(0, 4096),
        Duration::from_secs(2),
        None,
    )
    .unwrap();
    assert!((r.net_rx.unwrap() - 2048.0).abs() < 1e-9);
}

#[test]
fn a_counter_that_goes_backwards_is_a_reset_not_a_rate() {
    // A recreated container starts its counters again from zero.
    assert!(
        rate(
            &counters(9_000_000_000, 10),
            &counters(1_000, 10),
            Duration::from_secs(1),
            None
        )
        .is_none()
    );
    assert!(
        rate(
            &counters(0, 5000),
            &counters(0, 10),
            Duration::from_secs(1),
            None
        )
        .is_none()
    );
    assert!(rate(&counters(0, 0), &counters(1, 0), Duration::ZERO, None).is_none());
}

#[test]
fn a_limit_as_large_as_the_host_is_no_limit() {
    // Docker reports the host's memory as the limit of an unlimited container.
    let mut unlimited = counters(0, 0);
    unlimited.mem_limit = 64 << 30;
    let r = rate(
        &unlimited,
        &unlimited,
        Duration::from_secs(1),
        Some(64 << 30),
    )
    .unwrap();
    assert_eq!(r.mem_limit, None);
    let limited = rate(
        &counters(0, 0),
        &counters(0, 0),
        Duration::from_secs(1),
        Some(64 << 30),
    )
    .unwrap();
    assert_eq!(limited.mem_limit, Some(512 << 20));
}

#[test]
fn throttling_is_the_share_of_periods_held_back() {
    let mut a = counters(0, 0);
    let mut b = counters(0, 0);
    a.periods = 100;
    b.periods = 200;
    a.throttled_periods = 10;
    b.throttled_periods = 35;
    let r = rate(&a, &b, Duration::from_secs(1), None).unwrap();
    assert!((r.throttled.unwrap() - 0.25).abs() < 1e-9);
    // No CPU limit: periods never advance, so there is no figure at all.
    assert_eq!(
        rate(
            &counters(0, 0),
            &counters(0, 0),
            Duration::from_secs(1),
            None
        )
        .unwrap()
        .throttled,
        None
    );
}

#[test]
fn an_accumulator_averages_and_keeps_the_peak() {
    let mut acc = Accumulator::new();
    for (cpu, mem) in [(0.2, 100), (1.8, 300), (0.4, 200)] {
        acc.add(&Reading {
            cpu: Some(cpu),
            cpu_max: Some(cpu),
            mem: Some(mem),
            mem_max: Some(mem),
            ..Reading::default()
        });
    }
    let r = acc.finish(60).unwrap();
    assert_eq!(r.t, 60);
    assert!((r.cpu.unwrap() - 0.8).abs() < 1e-9);
    assert!((r.cpu_max.unwrap() - 1.8).abs() < 1e-9);
    assert_eq!(r.mem, Some(200));
    assert_eq!(r.mem_max, Some(300));
    assert!(
        acc.is_empty() && acc.finish(120).is_none(),
        "finishing empties it"
    );
}

#[test]
fn thinning_keeps_the_spike() {
    let points: Vec<Reading> = (0..10_000)
        .map(|i| {
            let cpu = if i == 7_777 { 12.0 } else { 0.1 };
            Reading {
                t: i * 60,
                cpu: Some(cpu),
                cpu_max: Some(cpu),
                ..Reading::default()
            }
        })
        .collect();
    let thin = thin(&points, 300);
    assert!(thin.len() <= 300);
    let peak = thin.iter().filter_map(|r| r.cpu_max).fold(0.0, f64::max);
    assert!((peak - 12.0).abs() < 1e-9, "the spike survives: {peak}");
    assert_eq!(thin.first().unwrap().t, 0);
}

#[test]
fn a_short_series_is_not_thinned() {
    let points = vec![
        Reading {
            t: 1,
            ..Reading::default()
        },
        Reading {
            t: 2,
            ..Reading::default()
        },
    ];
    assert_eq!(thin(&points, 300), points);
}

#[test]
fn a_stack_sums_its_containers() {
    let a = Reading {
        cpu: Some(0.5),
        cpu_max: Some(1.0),
        mem: Some(100),
        mem_max: Some(150),
        mem_limit: Some(200),
        ..Reading::default()
    };
    let b = Reading {
        cpu: Some(0.25),
        cpu_max: Some(0.5),
        mem: Some(50),
        mem_max: Some(60),
        mem_limit: None,
        ..Reading::default()
    };
    let s = sum_at(30, &[a, b]);
    assert_eq!(s.t, 30);
    assert!((s.cpu.unwrap() - 0.75).abs() < 1e-9);
    assert_eq!(s.mem, Some(150));
    assert_eq!(
        s.mem_limit, None,
        "one unlimited container makes the stack unlimited"
    );
}

const STAT: &str = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 50 0 25 400 25 0 0 0 0 0\ncpu1 50 0 25 400 25 0 0 0 0 0\nintr 12345\n";

#[test]
fn proc_stat_gives_busy_and_total_time_and_the_cpu_count() {
    // total = user..steal = 100+0+50+800+50+0+0+0; idle = idle + iowait.
    assert_eq!(parse_proc_stat(STAT), Some((150, 1000, 2)));
    assert_eq!(parse_proc_stat("nonsense"), None);
}

#[test]
fn meminfo_gives_total_and_available_in_bytes() {
    let text =
        "MemTotal:       65565160 kB\nMemFree:         2332560 kB\nMemAvailable:   40706140 kB\n";
    assert_eq!(
        parse_meminfo(text),
        Some((65_565_160 * 1024, 40_706_140 * 1024))
    );
}

#[test]
fn loadavg_gives_the_one_minute_figure() {
    assert_eq!(parse_loadavg("0.15 0.09 0.07 3/4689 3123\n"), Some(0.15));
}

#[test]
fn net_dev_lists_interfaces_without_loopback() {
    let text = "Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    lo: 100 1 0 0 0 0 0 0 100 1 0 0 0 0 0 0\n  eth0: 5000 10 0 0 0 0 0 0 7000 12 0 0 0 0 0 0\n";
    assert_eq!(parse_net_dev(text), vec![("eth0".to_owned(), 5000, 7000)]);
}

#[test]
fn host_cpu_is_busy_share_times_cpus_and_memory_excludes_cache() {
    let a = HostCounters {
        cpu_busy: 100,
        cpu_total: 1000,
        cpus: 4,
        mem_total: 8 << 30,
        mem_available: 6 << 30,
        load1: Some(0.5),
    };
    let b = HostCounters {
        cpu_busy: 300,
        cpu_total: 2000,
        cpus: 4,
        mem_total: 8 << 30,
        mem_available: 5 << 30,
        load1: Some(0.7),
    };
    let r = host_rate(&a, &b).unwrap();
    assert!((r.cpu.unwrap() - 0.8).abs() < 1e-9, "200/1000 of 4 cpus");
    assert_eq!(r.mem, Some(3 << 30));
    assert_eq!(r.mem_limit, Some(8 << 30));
    assert_eq!(r.load, Some(0.7));
}

#[test]
fn net_dev_leaves_out_container_plumbing() {
    // Each container's veth and each compose network's bridge come and go
    // with deploys, under new names; containers' traffic is measured per
    // container already.
    let text = "h\nh\n  eth0: 1 0 0 0 0 0 0 0 2 0 0 0 0 0 0 0\n veth1a2b3c: 1 0 0 0 0 0 0 0 2 0 0 0 0 0 0 0\nbr-0123abcd: 1 0 0 0 0 0 0 0 2 0 0 0 0 0 0 0\n  docker0: 1 0 0 0 0 0 0 0 2 0 0 0 0 0 0 0\n    wg0: 1 0 0 0 0 0 0 0 2 0 0 0 0 0 0 0\n";
    let names: Vec<String> = parse_net_dev(text).into_iter().map(|n| n.0).collect();
    assert_eq!(names, ["eth0", "docker0", "wg0"]);
}

#[test]
fn a_pair_read_back_to_back_is_no_rate() {
    // A starved sampler reads two buffered messages a second apart in
    // counters but microseconds apart in arrival: a second of CPU over
    // microseconds would be thousands of cores, kept for a year.
    let prev = Counters {
        cpu_ns: 0,
        ..Counters::default()
    };
    let next = Counters {
        cpu_ns: 1_000_000_000,
        ..Counters::default()
    };
    assert!(rate(&prev, &next, Duration::from_micros(50), None).is_none());
    assert!(rate(&prev, &next, Duration::from_millis(400), None).is_none());
    assert!(rate(&prev, &next, Duration::from_millis(1000), None).is_some());
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn a_containers_figures_are_its_95th_percentile_and_its_peak() {
    let points: Vec<Reading> = (0..100_u64)
        .map(|i| Reading {
            t: i as i64 * 60,
            cpu: Some(i as f64 / 100.0),
            cpu_max: Some(if i == 50 { 3.0 } else { i as f64 / 100.0 }),
            mem: Some(i * 10),
            mem_max: Some(i * 10 + 5),
            ..Reading::default()
        })
        .collect();
    let f = domain::metrics::figures("web-1", Some("web"), &points);
    assert_eq!(
        (f.key.as_str(), f.service.as_deref()),
        ("web-1", Some("web"))
    );
    // Nearest rank of 100 values at 0.95 is the 95th smallest (index 94).
    assert_eq!(f.cpu_typical, Some(94.0 / 100.0));
    assert_eq!(f.cpu_peak, Some(3.0), "a spike inside a minute counts");
    assert_eq!((f.mem_typical, f.mem_peak), (Some(940), Some(995)));

    let idle = domain::metrics::figures("idle-1", None, &[]);
    assert_eq!(
        (
            idle.cpu_typical,
            idle.cpu_peak,
            idle.mem_typical,
            idle.mem_peak
        ),
        (None, None, None, None)
    );
}

/// A quarter-hour boundary: 2027-01-15 08:00:00 UTC.
const QUARTER: i64 = 1_800_000_000;

#[test]
fn a_quarter_hour_is_rolled_up_once_it_has_ended() {
    assert_eq!(QUARTER % QUARTER_SECS, 0);
    // Minutes written up to 10:14 end at 10:15, a quarter boundary.
    let q = QUARTER;
    assert_eq!(quarter_due(q, Some(q - 900)), Some((q - 900, q)));
    // Mid-quarter, with the last one done: nothing.
    assert_eq!(quarter_due(q + 300, Some(q)), None);
}

#[test]
fn a_boundary_missed_by_a_late_minute_is_still_rolled_up() {
    // The minute loop slips when writing takes long; the boundary it steps
    // over must not be skipped.
    let q = QUARTER;
    assert_eq!(quarter_due(q + 60, Some(q - 900)), Some((q - 900, q)));
    // Several missed at once are caught up together.
    assert_eq!(quarter_due(q + 60, Some(q - 2700)), Some((q - 2700, q)));
}

#[test]
fn after_a_start_the_last_whole_quarter_is_rolled_up_again() {
    // Rolling up is idempotent, and the quarter before a restart may never
    // have been.
    let q = QUARTER;
    assert_eq!(quarter_due(q + 420, None), Some((q - 900, q)));
}
