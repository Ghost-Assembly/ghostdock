use server::metrics::Sampler;
use server::metrics::host::{HostPaths, HostReader, read_host};
use shared::metrics::{Reading, SubjectKind, Target};

fn cpu(v: f64) -> Reading {
    Reading {
        cpu: Some(v),
        cpu_max: Some(v),
        mem: Some(100),
        mem_max: Some(100),
        ..Reading::default()
    }
}

#[test]
fn a_tick_turns_readings_into_a_snapshot_with_stack_totals() {
    let s = Sampler::new(None, HostPaths::default());
    s.observe(
        SubjectKind::Container,
        "blog-web-1",
        Some("blog"),
        Some("web"),
        &cpu(0.2),
    );
    s.observe(
        SubjectKind::Container,
        "blog-web-1",
        Some("blog"),
        Some("web"),
        &cpu(0.4),
    );
    s.observe(
        SubjectKind::Container,
        "blog-db-1",
        Some("blog"),
        Some("db"),
        &cpu(1.0),
    );
    let now = s.tick(1_000);
    assert_eq!(now.at, 1_000);
    let web = now
        .containers
        .iter()
        .find(|c| c.key == "blog-web-1")
        .unwrap();
    assert!((web.reading.cpu.unwrap() - 0.3).abs() < 1e-9);
    let blog = now.stacks.iter().find(|s| s.project == "blog").unwrap();
    assert!((blog.reading.cpu.unwrap() - 1.3).abs() < 1e-9);
    assert_eq!(blog.cpu_hour.len(), 60);
}

#[test]
fn a_container_that_stops_reporting_leaves_the_snapshot() {
    let s = Sampler::new(None, HostPaths::default());
    s.observe(
        SubjectKind::Container,
        "gone-1",
        Some("gone"),
        None,
        &cpu(0.1),
    );
    s.tick(1_000);
    assert!(
        s.tick(1_005).containers.iter().any(|c| c.key == "gone-1"),
        "within grace"
    );
    assert!(
        !s.tick(1_020).containers.iter().any(|c| c.key == "gone-1"),
        "after 15 s silent"
    );
}

#[test]
fn the_live_ring_holds_one_hour() {
    let s = Sampler::new(None, HostPaths::default());
    for i in 0..800 {
        s.observe(SubjectKind::Host, "host", None, None, &cpu(0.1));
        s.tick(i * 5);
    }
    let ring = s.ring(&Target::host());
    assert_eq!(ring.len(), 720);
    assert_eq!(ring.last().unwrap().t, 799 * 5);
}

#[test]
fn a_minute_folds_its_five_second_points() {
    let s = Sampler::new(None, HostPaths::default());
    for (i, v) in [0.1, 0.5, 3.0].into_iter().enumerate() {
        s.observe(SubjectKind::Container, "x-1", Some("x"), None, &cpu(v));
        s.tick(i as i64 * 5);
    }
    let rows = s.take_minute(0);
    let row = rows.iter().find(|r| r.key == "x-1").unwrap();
    assert!((row.reading.cpu_max.unwrap() - 3.0).abs() < 1e-9);
    assert!((row.reading.cpu.unwrap() - 1.2).abs() < 1e-9);
    assert!(s.take_minute(60).is_empty(), "taken once");
}

#[test]
fn containers_are_marked_unavailable_without_a_daemon() {
    let s = Sampler::new(None, HostPaths::default());
    s.set_unavailable(true);
    assert!(s.tick(5).containers_unavailable);
}

#[tokio::test]
async fn host_figures_come_from_proc_and_optional_mounts() {
    let dir = tempfile::tempdir().unwrap();
    let proc_dir = dir.path().join("proc");
    std::fs::create_dir_all(&proc_dir).unwrap();
    std::fs::write(
        proc_dir.join("stat"),
        "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 100 0 50 800 50 0 0 0 0 0\n",
    )
    .unwrap();
    std::fs::write(
        proc_dir.join("meminfo"),
        "MemTotal: 1000 kB\nMemAvailable: 400 kB\n",
    )
    .unwrap();
    std::fs::write(proc_dir.join("loadavg"), "0.5 0.4 0.3 1/2 3\n").unwrap();
    let host_proc = dir.path().join("host-proc");
    std::fs::create_dir_all(host_proc.join("1/net")).unwrap();
    std::fs::write(
        host_proc.join("1/net/dev"),
        "h\nh\n  eno1: 10 0 0 0 0 0 0 0 20 0 0 0 0 0 0 0\n",
    )
    .unwrap();
    let disks = dir.path().join("disks");
    std::fs::create_dir_all(disks.join("media")).unwrap();

    let snap = read_host(HostPaths {
        proc: proc_dir,
        host_proc,
        disks,
        root: dir.path().to_path_buf(),
    })
    .await;
    let c = snap.counters.unwrap();
    assert_eq!((c.cpu_total, c.cpus), (1000, 1));
    assert_eq!(c.mem_total, 1000 * 1024);
    assert_eq!(snap.nets, vec![("eno1".to_owned(), 10, 20)]);
    let keys: Vec<_> = snap.disks.iter().map(|d| d.0.as_str()).collect();
    assert!(
        keys.contains(&"/") && keys.iter().any(|k| k.ends_with("media")),
        "{keys:?}"
    );
    assert!(
        snap.disks
            .iter()
            .all(|(_, used, cap)| used <= cap && *cap > 0)
    );
}

#[tokio::test]
async fn without_optional_mounts_host_figures_still_come() {
    let snap = read_host(HostPaths {
        host_proc: "/nonexistent/host-proc".into(),
        disks: "/nonexistent/disks".into(),
        ..HostPaths::default()
    })
    .await;
    assert!(snap.counters.is_some(), "this machine's /proc");
    assert!(snap.nets.is_empty());
    assert_eq!(snap.disks.len(), 1, "only the root disk");
}

#[tokio::test]
async fn a_slow_disk_does_not_hold_up_the_tick() {
    // A pipe nobody writes to: reading it blocks, as a hung mount does.
    let dir = tempfile::tempdir().unwrap();
    let proc_dir = dir.path().join("proc");
    std::fs::create_dir_all(&proc_dir).unwrap();
    let fifo = proc_dir.join("stat");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let started = std::time::Instant::now();
    let snap = read_host(HostPaths {
        proc: proc_dir,
        ..HostPaths::default()
    })
    .await;
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "gave up at the deadline"
    );
    assert!(snap.counters.is_none());
    // Release the stuck reader: the test's runtime waits for it on shutdown.
    drop(std::fs::OpenOptions::new().write(true).open(&fifo).unwrap());
}

#[test]
fn the_live_hour_holds_nothing_older_than_an_hour() {
    // A container that stopped long ago is not "the last hour".
    let s = Sampler::new(None, HostPaths::default());
    s.observe(
        SubjectKind::Container,
        "gone-1",
        Some("gone"),
        None,
        &cpu(0.5),
    );
    s.tick(0);
    for i in 1..=740 {
        s.observe(SubjectKind::Host, "host", None, None, &cpu(0.1));
        s.tick(i * 5);
    }
    assert!(
        s.ring(&Target::Subject(
            SubjectKind::Container,
            "gone-1".to_owned()
        ))
        .is_empty()
    );
    assert!(s.ring(&Target::Stack("gone".to_owned())).is_empty());
    assert_eq!(s.ring(&Target::host()).len(), 720);
}

static RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static STALE_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Disk space, except that the disk named "stale" hangs as a dead network
/// share does, until the test lets it go.
fn space_with_a_stale_share(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::sync::atomic::Ordering;
    if path.ends_with("stale") {
        STALE_READS.fetch_add(1, Ordering::SeqCst);
        while !RELEASE.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        return None;
    }
    Some((1, 2))
}

/// Lets the stuck read go when the test ends, passing or failing: the
/// runtime waits for blocked threads on shutdown.
struct ReleaseOnDrop;

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        RELEASE.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[tokio::test]
async fn a_hung_disk_costs_only_its_own_figure() {
    use std::sync::atomic::Ordering;
    let _release = ReleaseOnDrop;
    let dir = tempfile::tempdir().unwrap();
    let proc_dir = dir.path().join("proc");
    std::fs::create_dir_all(&proc_dir).unwrap();
    std::fs::write(
        proc_dir.join("stat"),
        "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 100 0 50 800 50 0 0 0 0 0\n",
    )
    .unwrap();
    std::fs::write(
        proc_dir.join("meminfo"),
        "MemTotal: 1000 kB\nMemAvailable: 400 kB\n",
    )
    .unwrap();
    let disks = dir.path().join("disks");
    std::fs::create_dir_all(disks.join("media")).unwrap();
    std::fs::create_dir_all(disks.join("stale")).unwrap();
    let paths = HostPaths {
        proc: proc_dir,
        disks,
        ..HostPaths::default()
    };

    let mut reader = HostReader::new(paths, space_with_a_stale_share).await;
    for pass in ["first", "second"] {
        let started = std::time::Instant::now();
        let snap = reader.read().await;
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "{pass}: gave up at the deadline"
        );
        assert!(snap.counters.is_some(), "{pass}: CPU and memory still come");
        let keys: Vec<_> = snap.disks.iter().map(|d| d.0.as_str()).collect();
        assert!(
            keys.contains(&"/") && keys.iter().any(|k| k.ends_with("media")),
            "{pass}: {keys:?}"
        );
        assert!(
            !keys.iter().any(|k| k.ends_with("stale")),
            "{pass}: {keys:?}"
        );
    }
    assert_eq!(
        STALE_READS.load(Ordering::SeqCst),
        1,
        "one stuck read, not one a tick"
    );
    let again = std::time::Instant::now();
    reader.read().await;
    assert!(
        again.elapsed() < std::time::Duration::from_millis(500),
        "a disk known to be stuck is not waited for again"
    );
}

#[test]
fn a_stacks_containers_over_the_live_hour_are_its_own() {
    let s = Sampler::new(None, HostPaths::default());
    for (i, v) in [0.1, 0.5, 3.0].into_iter().enumerate() {
        s.observe(
            SubjectKind::Container,
            "blog-web-1",
            Some("blog"),
            Some("web"),
            &cpu(v),
        );
        s.observe(
            SubjectKind::Container,
            "shop-web-1",
            Some("shop"),
            Some("web"),
            &cpu(9.0),
        );
        s.tick(i as i64 * 5);
    }
    let figures = s.ring_figures("blog");
    assert_eq!(figures.len(), 1, "{figures:?}");
    assert_eq!(figures[0].key, "blog-web-1");
    assert_eq!(figures[0].cpu_peak, Some(3.0));
    assert!(s.ring_figures("nothing").is_empty());
}
