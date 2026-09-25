use shared::metrics::{Reading, Resolution, SubjectKind};
use store::metrics::MetricsStore;

fn r(t: i64, cpu: f64, mem: u64) -> Reading {
    Reading {
        t,
        cpu: Some(cpu),
        cpu_max: Some(cpu),
        mem: Some(mem),
        mem_max: Some(mem),
        ..Reading::default()
    }
}

#[tokio::test]
async fn a_damaged_file_is_told_apart_from_one_that_cannot_be_opened() {
    // A damaged history is set aside and started afresh. Anything else (a
    // lock, a permission, a full disk) must not cost the history.
    let dir = tempfile::tempdir().unwrap();
    let damaged = dir.path().join("metrics.db");
    std::fs::write(&damaged, vec![0x5a_u8; 8192]).unwrap();
    let e = MetricsStore::open(damaged.to_str().unwrap())
        .await
        .expect_err("not a database");
    assert!(e.is_corrupt(), "{e}");

    let unreachable = dir.path().join("missing/dir/metrics.db");
    let e = MetricsStore::open(unreachable.to_str().unwrap())
        .await
        .expect_err("no such directory");
    assert!(!e.is_corrupt(), "{e}");
}

#[tokio::test]
async fn minutes_are_written_and_read_back_in_order() {
    let m = MetricsStore::open_in_memory().await.unwrap();
    let id = m
        .subject(
            SubjectKind::Container,
            "blog-web-1",
            Some("blog"),
            Some("web"),
            0,
        )
        .await
        .unwrap();
    m.write_minute(&[(id, r(120, 0.2, 10)), (id, r(60, 0.1, 5))])
        .await
        .unwrap();
    let got = m.read(id, Resolution::Minute, 0, 1000).await.unwrap();
    assert_eq!(got.iter().map(|p| p.t).collect::<Vec<_>>(), [60, 120]);
    assert_eq!(got[1].mem, Some(10));
}

#[tokio::test]
async fn history_continues_across_a_recreated_container() {
    // A deploy recreates the container: new id, same name, same subject.
    let m = MetricsStore::open_in_memory().await.unwrap();
    let before = m
        .subject(SubjectKind::Container, "blog-web-1", Some("blog"), None, 0)
        .await
        .unwrap();
    let after = m
        .subject(
            SubjectKind::Container,
            "blog-web-1",
            Some("blog"),
            None,
            600,
        )
        .await
        .unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
async fn quarter_hours_fold_their_minutes() {
    let m = MetricsStore::open_in_memory().await.unwrap();
    let id = m
        .subject(SubjectKind::Host, "host", None, None, 0)
        .await
        .unwrap();
    let rows: Vec<_> = (0..15)
        .map(|i| (id, r(i * 60, if i == 7 { 4.0 } else { 1.0 }, 100)))
        .collect();
    m.write_minute(&rows).await.unwrap();
    assert_eq!(m.rollup(0, 900).await.unwrap(), 1);
    let q = m.read(id, Resolution::Quarter, 0, 900).await.unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].t, 0);
    assert!((q[0].cpu.unwrap() - 1.2).abs() < 1e-9, "the average");
    assert!(
        (q[0].cpu_max.unwrap() - 4.0).abs() < 1e-9,
        "the peak survives"
    );
    // Rolling up again replaces rather than duplicates.
    m.rollup(0, 900).await.unwrap();
    assert_eq!(
        m.read(id, Resolution::Quarter, 0, 900).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn a_stack_is_its_containers_summed_minute_by_minute() {
    let m = MetricsStore::open_in_memory().await.unwrap();
    let a = m
        .subject(SubjectKind::Container, "blog-web-1", Some("blog"), None, 0)
        .await
        .unwrap();
    let b = m
        .subject(SubjectKind::Container, "blog-db-1", Some("blog"), None, 0)
        .await
        .unwrap();
    let c = m
        .subject(SubjectKind::Container, "wiki-app-1", Some("wiki"), None, 0)
        .await
        .unwrap();
    m.write_minute(&[
        (a, r(60, 0.5, 100)),
        (b, r(60, 0.25, 50)),
        (c, r(60, 9.0, 999)),
    ])
    .await
    .unwrap();
    let s = m
        .read_stack("blog", Resolution::Minute, 0, 1000)
        .await
        .unwrap();
    assert_eq!(s.len(), 1);
    assert!((s[0].cpu.unwrap() - 0.75).abs() < 1e-9);
    assert_eq!(s[0].mem, Some(150));
}

#[tokio::test]
async fn pruning_keeps_what_is_inside_retention() {
    let m = MetricsStore::open_in_memory().await.unwrap();
    let id = m
        .subject(SubjectKind::Host, "host", None, None, 0)
        .await
        .unwrap();
    let day = 86_400;
    let now = 400 * day;
    m.write_minute(&[
        (id, r(now - 31 * day, 1.0, 1)),
        (id, r(now - 29 * day, 1.0, 1)),
    ])
    .await
    .unwrap();
    m.subject(SubjectKind::Host, "host", None, None, now)
        .await
        .unwrap();
    m.prune(now).await.unwrap();
    let left = m.read(id, Resolution::Minute, 0, now).await.unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].t, now - 29 * day);
}

#[tokio::test]
async fn events_are_counted_per_subject() {
    let m = MetricsStore::open_in_memory().await.unwrap();
    let id = m
        .subject(SubjectKind::Container, "x", None, None, 0)
        .await
        .unwrap();
    m.record_event(id, 100, "oom").await.unwrap();
    m.record_event(id, 200, "oom").await.unwrap();
    m.record_event(id, 300, "restart").await.unwrap();
    assert_eq!(m.count_events(id, "oom", 150).await.unwrap(), 1);
    assert_eq!(m.count_events(id, "oom", 0).await.unwrap(), 2);
}

#[tokio::test]
async fn a_months_minutes_come_back_as_300_points_quickly_with_the_spike() {
    // A chart shows at most 300 points, so that is what is read: buckets of
    // time, averaged, with peaks kept, made by SQLite rather than by
    // decoding 43,200 rows one by one.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metrics.db");
    let m = MetricsStore::open(path.to_str().unwrap()).await.unwrap();
    let id = m
        .subject(SubjectKind::Container, "busy-1", Some("busy"), None, 0)
        .await
        .unwrap();
    let other = m
        .subject(SubjectKind::Container, "busy-2", Some("busy"), None, 0)
        .await
        .unwrap();
    for day in 0..30 {
        let rows: Vec<_> = (0..1440)
            .flat_map(|i| {
                let t = (day * 1440 + i) * 60;
                let cpu = if day == 17 && i == 700 { 9.0 } else { 0.5 };
                [(id, r(t, cpu, 1)), (other, r(t, 0.5, 1))]
            })
            .collect();
        m.write_minute(&rows).await.unwrap();
    }
    let month = 30 * 86_400;
    let started = std::time::Instant::now();
    let got = m
        .read_series(id, Resolution::Minute, 0, month, 300)
        .await
        .unwrap();
    let took = started.elapsed();
    assert!(!got.is_empty() && got.len() <= 300, "{}", got.len());
    let peak = got.iter().filter_map(|p| p.cpu_max).fold(0.0, f64::max);
    assert!((peak - 9.0).abs() < 1e-9, "the spike survives: {peak}");
    assert!(
        (got[0].cpu.unwrap() - 0.5).abs() < 1e-9,
        "averages stay averages"
    );
    assert!(took < std::time::Duration::from_millis(50), "took {took:?}");

    let stack = m
        .read_stack_series("busy", Resolution::Minute, 0, month, 300)
        .await
        .unwrap();
    assert!(!stack.is_empty() && stack.len() <= 300);
    assert!(
        (stack[0].cpu.unwrap() - 1.0).abs() < 1e-9,
        "two containers at 0.5"
    );
}

#[allow(clippy::cast_precision_loss)]
fn varied(i: i64) -> Reading {
    Reading {
        t: i * 60,
        // Some minutes have no CPU figure, a few have nothing at all.
        cpu: (i % 97 != 0 && i % 211 != 0).then_some((i % 13) as f64 * 0.1),
        cpu_max: (i % 211 != 0).then_some((i % 17) as f64 * 0.15),
        mem: (i % 211 != 0).then(|| (100 << 20) + ((i % 7) as u64) * 4096),
        mem_max: (i % 211 != 0).then(|| (100 << 20) + ((i % 11) as u64) * 8192),
        mem_limit: Some(if i < 4000 { 512 << 20 } else { 1 << 30 }),
        throttled: (i % 5 == 0).then_some(0.25),
        ..Reading::default()
    }
}

#[tokio::test]
async fn sizing_inputs_from_sql_match_the_rules_own_summary() {
    let m = MetricsStore::open_in_memory().await.unwrap();
    let id = m
        .subject(
            SubjectKind::Container,
            "blog-web-1",
            Some("blog"),
            Some("web"),
            0,
        )
        .await
        .unwrap();
    let other = m
        .subject(
            SubjectKind::Container,
            "blog-db-1",
            Some("blog"),
            Some("db"),
            0,
        )
        .await
        .unwrap();
    let rows: Vec<Reading> = (0..5000).map(varied).collect();
    let mut batch: Vec<_> = rows.iter().map(|r| (id, *r)).collect();
    // Another container, and this one outside the window: neither counts.
    batch.push((other, r(600, 9.0, 1 << 40)));
    batch.push((id, r(5000 * 60, 9.0, 1 << 40)));
    m.write_minute(&batch).await.unwrap();

    let got = m.sizing_summary(id, 0, 5000 * 60).await.unwrap();
    assert_eq!(got, domain::sizing::summarise(&rows));
    assert_eq!(got.mem_limit, Some(1 << 30), "the limit in force last");
}

#[tokio::test]
async fn a_months_sizing_inputs_are_summarised_quickly() {
    let dir = tempfile::tempdir().unwrap();
    let m = MetricsStore::open(dir.path().join("metrics.db").to_str().unwrap())
        .await
        .unwrap();
    let id = m
        .subject(SubjectKind::Container, "busy-1", Some("busy"), None, 0)
        .await
        .unwrap();
    for day in 0..30 {
        let rows: Vec<_> = (0..1440).map(|i| (id, varied(day * 1440 + i))).collect();
        m.write_minute(&rows).await.unwrap();
    }
    let started = std::time::Instant::now();
    let got = m.sizing_summary(id, 0, 30 * 86_400).await.unwrap();
    let took = started.elapsed();
    assert!(got.running > 40_000);
    // Two sorts of a month of figures, about 40 ms; reading the rows into
    // Rust instead takes several times that. The server also caches advice.
    assert!(
        took < std::time::Duration::from_millis(100),
        "took {took:?}"
    );
}

#[tokio::test]
async fn a_stacks_container_figures_from_sql_match_the_definition() {
    let m = MetricsStore::open_in_memory().await.unwrap();
    let web = m
        .subject(
            SubjectKind::Container,
            "blog-web-1",
            Some("blog"),
            Some("web"),
            0,
        )
        .await
        .unwrap();
    let db = m
        .subject(
            SubjectKind::Container,
            "blog-db-1",
            Some("blog"),
            Some("db"),
            0,
        )
        .await
        .unwrap();
    let other = m
        .subject(
            SubjectKind::Container,
            "shop-web-1",
            Some("shop"),
            Some("web"),
            0,
        )
        .await
        .unwrap();
    let web_rows: Vec<Reading> = (0..3000).map(varied).collect();
    let db_rows: Vec<Reading> = (0..1000).map(|i| varied(i * 3 + 1)).collect();
    let mut batch: Vec<_> = web_rows.iter().map(|r| (web, *r)).collect();
    batch.extend(db_rows.iter().map(|r| (db, *r)));
    batch.push((other, r(600, 9.0, 1 << 40)));
    m.write_minute(&batch).await.unwrap();

    let got = m
        .container_figures("blog", Resolution::Minute, 0, 1_000_000)
        .await
        .unwrap();
    assert_eq!(
        got,
        vec![
            domain::metrics::figures("blog-db-1", Some("db"), &db_rows),
            domain::metrics::figures("blog-web-1", Some("web"), &web_rows),
        ]
    );
}
