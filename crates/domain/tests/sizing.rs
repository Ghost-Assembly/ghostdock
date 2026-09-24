use domain::sizing::{History, recommend};
use shared::metrics::{Reading, Severity};

const MIB: u64 = 1 << 20;

fn minutes(
    days: usize,
    cpu: impl Fn(usize) -> f64,
    mem: impl Fn(usize) -> u64,
    limit: Option<u64>,
) -> Vec<Reading> {
    (0..days * 1440)
        .map(|i| Reading {
            t: i as i64 * 60,
            cpu: Some(cpu(i)),
            cpu_max: Some(cpu(i)),
            mem: Some(mem(i)),
            mem_max: Some(mem(i)),
            mem_limit: limit,
            throttled: None,
            ..Reading::default()
        })
        .collect()
}

fn history<'a>(minutes: &'a [Reading], ooms: u32) -> History<'a> {
    History {
        container: "blog-web-1",
        project: Some("blog"),
        service: Some("web"),
        minutes,
        ooms,
    }
}

#[test]
fn too_little_history_suggests_nothing_and_says_why() {
    let m = minutes(2, |_| 0.1, |_| 100 * MIB, None);
    let r = recommend(&history(&m, 0));
    assert_eq!((r.cpus, r.memory, r.snippet.as_deref()), (None, None, None));
    assert!(r.evidence.contains("3 days"), "{}", r.evidence);
}

#[test]
fn memory_is_the_peak_with_headroom_rounded_to_64_mib() {
    let m = minutes(
        21,
        |_| 0.1,
        |i| if i == 5000 { 412 * MIB } else { 300 * MIB },
        None,
    );
    let r = recommend(&history(&m, 0));
    // 412 × 1.3 = 535.6 MiB → 576 MiB.
    assert_eq!(r.memory, Some(576 * MIB));
    assert!(
        r.evidence.contains("21 days") && r.evidence.contains("412 MiB"),
        "{}",
        r.evidence
    );
}

#[test]
fn cpu_is_the_95th_percentile_with_headroom_and_bursts_are_mentioned() {
    let m = minutes(
        10,
        |i| if i % 100 == 0 { 1.8 } else { 0.2 },
        |_| 100 * MIB,
        None,
    );
    let r = recommend(&history(&m, 0));
    // p95 is 0.2 → 0.3 cores.
    assert!((r.cpus.unwrap() - 0.3).abs() < 1e-9, "{:?}", r.cpus);
    assert!(r.evidence.contains("bursts to 1.80"), "{}", r.evidence);
}

#[test]
fn after_an_oom_kill_the_old_limit_is_not_trusted() {
    let m = minutes(7, |_| 0.1, |_| 250 * MIB, Some(256 * MIB));
    let r = recommend(&history(&m, 3));
    assert!(
        r.memory.unwrap() >= 384 * MIB,
        "at least 1.5 × the limit: {:?}",
        r.memory
    );
    assert_eq!(r.flags[0].severity, Severity::Bad);
    assert!(r.flags[0].text.contains("3 times"));
}

#[test]
fn flags_cover_limits_throttling_and_waste() {
    let near = minutes(7, |_| 0.1, |_| 470 * MIB, Some(512 * MIB));
    assert!(
        recommend(&history(&near, 0))
            .flags
            .iter()
            .any(|f| f.severity == Severity::Degraded)
    );

    let unlimited = minutes(7, |_| 0.1, |_| 100 * MIB, None);
    assert!(
        recommend(&history(&unlimited, 0))
            .flags
            .iter()
            .any(|f| f.severity == Severity::Info && f.text.contains("no memory limit"))
    );

    let wasteful = minutes(7, |_| 0.1, |_| 100 * MIB, Some(2048 * MIB));
    assert!(
        recommend(&history(&wasteful, 0))
            .flags
            .iter()
            .any(|f| f.text.contains("never used more than"))
    );

    let mut throttled = minutes(7, |_| 0.5, |_| 100 * MIB, Some(512 * MIB));
    throttled.iter_mut().for_each(|r| r.throttled = Some(0.25));
    assert!(
        recommend(&history(&throttled, 0))
            .flags
            .iter()
            .any(|f| f.text.contains("throttled"))
    );
}

#[test]
fn the_snippet_names_the_service_and_uses_compose_units() {
    let m = minutes(7, |_| 0.2, |_| 300 * MIB, None);
    let snippet = recommend(&history(&m, 0)).snippet.unwrap();
    assert!(snippet.contains("  web:\n"), "{snippet}");
    assert!(snippet.contains("cpus: \"0.30\""), "{snippet}");
    assert!(snippet.contains("memory: 448M"), "{snippet}");
}

#[test]
fn a_single_oom_kill_reads_once() {
    let m = minutes(7, |_| 0.1, |_| 250 * MIB, Some(256 * MIB));
    assert_eq!(
        recommend(&history(&m, 1)).flags[0].text,
        "OOM-killed once in the last 30 days"
    );
}
