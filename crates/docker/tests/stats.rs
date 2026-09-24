use std::collections::HashMap;

use bollard::models::{
    ContainerBlkioStatEntry, ContainerBlkioStats, ContainerCpuStats, ContainerCpuUsage,
    ContainerMemoryStats, ContainerNetworkStats, ContainerStatsResponse, ContainerThrottlingData,
};
use docker::stats::to_counters;

fn sample() -> ContainerStatsResponse {
    ContainerStatsResponse {
        cpu_stats: Some(ContainerCpuStats {
            cpu_usage: Some(ContainerCpuUsage {
                total_usage: Some(5_000_000_000),
                ..Default::default()
            }),
            throttling_data: Some(ContainerThrottlingData {
                periods: Some(100),
                throttled_periods: Some(7),
                ..Default::default()
            }),
            ..Default::default()
        }),
        memory_stats: Some(ContainerMemoryStats {
            usage: Some(300 << 20),
            limit: Some(512 << 20),
            stats: Some(HashMap::from([("inactive_file".to_owned(), 100 << 20)])),
            ..Default::default()
        }),
        networks: Some(HashMap::from([
            (
                "eth0".to_owned(),
                ContainerNetworkStats {
                    rx_bytes: Some(1000),
                    tx_bytes: Some(2000),
                    ..Default::default()
                },
            ),
            (
                "eth1".to_owned(),
                ContainerNetworkStats {
                    rx_bytes: Some(10),
                    tx_bytes: Some(20),
                    ..Default::default()
                },
            ),
        ])),
        blkio_stats: Some(ContainerBlkioStats {
            io_service_bytes_recursive: Some(vec![
                ContainerBlkioStatEntry {
                    op: Some("read".to_owned()),
                    value: Some(4096),
                    ..Default::default()
                },
                ContainerBlkioStatEntry {
                    op: Some("Write".to_owned()),
                    value: Some(8192),
                    ..Default::default()
                },
                ContainerBlkioStatEntry {
                    op: Some("Total".to_owned()),
                    value: Some(99_999),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn counters_come_from_the_daemons_figures() {
    let c = to_counters(&sample()).unwrap();
    assert_eq!(c.cpu_ns, 5_000_000_000);
    assert_eq!(
        c.mem_used,
        200 << 20,
        "page cache that can be reclaimed is not in use"
    );
    assert_eq!(c.mem_limit, 512 << 20);
    assert_eq!((c.net_rx, c.net_tx), (1010, 2020), "every interface counts");
    assert_eq!(
        (c.io_read, c.io_write),
        (4096, 8192),
        "reads and writes, not the total"
    );
    assert_eq!((c.periods, c.throttled_periods), (100, 7));
}

#[test]
fn cgroup_v1_names_its_cache_differently() {
    let mut s = sample();
    s.memory_stats.as_mut().unwrap().stats = Some(HashMap::from([(
        "total_inactive_file".to_owned(),
        50 << 20,
    )]));
    assert_eq!(to_counters(&s).unwrap().mem_used, 250 << 20);
}

#[test]
fn a_stopped_containers_empty_figures_are_no_counters() {
    assert!(to_counters(&ContainerStatsResponse::default()).is_none());
}

#[test]
fn a_stopped_containers_zeroed_figures_are_no_counters() {
    // What the daemon actually streams for a stopped container: CPU time
    // present but zero, memory and process counts empty.
    let s = ContainerStatsResponse {
        cpu_stats: Some(ContainerCpuStats {
            cpu_usage: Some(ContainerCpuUsage {
                total_usage: Some(0),
                usage_in_kernelmode: Some(0),
                usage_in_usermode: Some(0),
                ..Default::default()
            }),
            throttling_data: Some(ContainerThrottlingData {
                periods: Some(0),
                throttled_periods: Some(0),
                throttled_time: Some(0),
            }),
            ..Default::default()
        }),
        memory_stats: Some(ContainerMemoryStats::default()),
        blkio_stats: Some(ContainerBlkioStats::default()),
        ..Default::default()
    };
    assert!(
        to_counters(&s).is_none(),
        "a stopped container is a gap, not zeros"
    );
}
