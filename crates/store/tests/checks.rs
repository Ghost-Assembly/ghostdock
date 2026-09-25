//! Checks, incidents, alert channels, rules and deliveries in ghostdock.db,
//! and check history in metrics.db.

use shared::alerts::{ChannelKind, Metric, NewAlertChannel, NewAlertRule};
use shared::checks::{Check, CheckKind};
use shared::metrics::Resolution;
use store::Store;
use store::hosts::LOCAL_HOST_ID;
use store::metrics::MetricsStore;

fn check(name: &str) -> Check {
    Check {
        id: 0,
        name: name.to_owned(),
        kind: CheckKind::Http,
        target: "https://blog.example.test/health".to_owned(),
        interval_s: 60,
        timeout_s: 10,
        retries: 2,
        expect_status_min: 200,
        expect_status_max: 399,
        keyword: Some("ok".to_owned()),
        latency_warn_ms: Some(800),
        stack_id: None,
        enabled: true,
        notify: true,
        created_at: chrono::DateTime::default(),
    }
}

#[tokio::test]
async fn a_check_is_stored_changed_and_removed() {
    let s = Store::open_in_memory().await.unwrap();
    let made = s.check_create(LOCAL_HOST_ID, &check("Blog")).await.unwrap();
    assert!(made.id > 0);
    assert_eq!(made.keyword.as_deref(), Some("ok"));
    assert!(made.created_at.timestamp() > 0);

    let mut changed = made.clone();
    changed.retries = 4;
    changed.keyword = None;
    let saved = s.check_update(&changed).await.unwrap();
    assert_eq!((saved.retries, saved.keyword.as_deref()), (4, None));
    assert_eq!(s.check_by_id(made.id).await.unwrap(), Some(saved));

    assert_eq!(s.checks_list(LOCAL_HOST_ID).await.unwrap().len(), 1);
    assert!(s.check_delete(made.id).await.unwrap());
    assert!(!s.check_delete(made.id).await.unwrap());
    assert_eq!(s.check_by_id(made.id).await.unwrap(), None);
}

#[tokio::test]
async fn two_checks_cannot_share_a_name() {
    let s = Store::open_in_memory().await.unwrap();
    s.check_create(LOCAL_HOST_ID, &check("Blog")).await.unwrap();
    let e = s
        .check_create(LOCAL_HOST_ID, &check("Blog"))
        .await
        .unwrap_err();
    assert!(matches!(e, store::Error::NameTaken), "{e}");
    let other = s.check_create(LOCAL_HOST_ID, &check("Shop")).await.unwrap();
    let mut renamed = other.clone();
    renamed.name = "Blog".to_owned();
    assert!(matches!(
        s.check_update(&renamed).await.unwrap_err(),
        store::Error::NameTaken
    ));
}

#[tokio::test]
async fn forgetting_a_stack_keeps_its_checks() {
    let s = Store::open_in_memory().await.unwrap();
    let stack = s
        .stack_create(LOCAL_HOST_ID, "blog", "Blog", "services: {}\n")
        .await
        .unwrap();
    let mut linked = check("Blog");
    linked.stack_id = Some(stack.id);
    let made = s.check_create(LOCAL_HOST_ID, &linked).await.unwrap();
    assert_eq!(made.stack_id, Some(stack.id));
    s.stack_delete(stack.id).await.unwrap();
    assert_eq!(
        s.check_by_id(made.id).await.unwrap().unwrap().stack_id,
        None
    );
}

#[tokio::test]
async fn an_incident_opens_once_closes_and_survives_a_restart() {
    let s = Store::open_in_memory().await.unwrap();
    let made = s.check_create(LOCAL_HOST_ID, &check("Blog")).await.unwrap();
    assert!(!s.incident_is_open(made.id).await.unwrap());
    s.incident_open(made.id, "connection refused", 1_000)
        .await
        .unwrap();
    assert!(s.incident_is_open(made.id).await.unwrap());
    s.incident_close(made.id, 1_300).await.unwrap();
    assert!(!s.incident_is_open(made.id).await.unwrap());
    s.incident_open(made.id, "answered 503", 2_000)
        .await
        .unwrap();

    let recent = s.incidents_recent(LOCAL_HOST_ID, 10).await.unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].cause, "answered 503", "newest first");
    assert_eq!(recent[0].ended_at, None);
    assert_eq!(recent[0].check_name, "Blog");
    assert_eq!(recent[1].ended_at.map(|t| t.timestamp()), Some(1_300));

    let since = s.incidents_for(made.id, 1_500).await.unwrap();
    assert_eq!(since.len(), 1, "only those still open or ended since");

    // Removing the check removes its incidents.
    s.check_delete(made.id).await.unwrap();
    assert!(
        s.incidents_recent(LOCAL_HOST_ID, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_channel_keeps_its_url_and_token_sealed_and_shows_only_the_host() {
    let s = Store::open_in_memory().await.unwrap();
    let url = "https://hooks.example.test/services/T0/B1/very-secret";
    let made = s
        .channel_create(&NewAlertChannel {
            name: "ops".to_owned(),
            kind: ChannelKind::Webhook,
            url: url.to_owned(),
            token: Some("tk_private".to_owned()),
        })
        .await
        .unwrap();
    assert_eq!(made.host, "hooks.example.test");
    assert_eq!(s.channels_list().await.unwrap(), vec![made.clone()]);

    // At rest, neither is in the clear.
    let (url_enc, token_enc): (String, Option<String>) =
        sqlx::query_as("SELECT url_enc, token_enc FROM alert_channels")
            .fetch_one(s.pool())
            .await
            .unwrap();
    assert!(!url_enc.contains("very-secret"));
    assert!(!token_enc.unwrap_or_default().contains("tk_private"));

    let open = s.channels_to_send().await.unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].url, url);
    assert_eq!(open[0].token.as_deref(), Some("tk_private"));

    assert!(matches!(
        s.channel_create(&NewAlertChannel {
            name: "ops".to_owned(),
            kind: ChannelKind::Ntfy,
            url: "https://ntfy.example.test/x".to_owned(),
            token: None,
        })
        .await
        .unwrap_err(),
        store::Error::NameTaken
    ));
    assert!(s.channel_delete(made.id).await.unwrap());
    assert!(s.channels_list().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_sealed_url_cannot_be_opened_as_a_token() {
    // Each field is sealed under its own purpose, so swapping the columns
    // in the database does not make a URL come out as a token.
    let s = Store::open_in_memory().await.unwrap();
    s.channel_create(&NewAlertChannel {
        name: "ops".to_owned(),
        kind: ChannelKind::Ntfy,
        url: "https://ntfy.example.test/topic".to_owned(),
        token: Some("tk".to_owned()),
    })
    .await
    .unwrap();
    sqlx::query("UPDATE alert_channels SET token_enc = url_enc")
        .execute(s.pool())
        .await
        .unwrap();
    assert!(s.channels_to_send().await.is_err());
}

#[tokio::test]
async fn rules_are_stored_changed_and_removed() {
    let s = Store::open_in_memory().await.unwrap();
    let rule = NewAlertRule {
        subject: "host".to_owned(),
        metric: Metric::Cpu,
        above_pct: 90.0,
        for_min: 5,
        enabled: true,
    };
    let made = s.rule_create(&rule).await.unwrap();
    assert_eq!(
        (made.metric, made.for_min, made.firing),
        (Metric::Cpu, 5, false)
    );
    let changed = s
        .rule_update(
            made.id,
            &NewAlertRule {
                above_pct: 75.0,
                ..rule.clone()
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert!((changed.above_pct - 75.0).abs() < 1e-9);
    assert_eq!(s.rules_list().await.unwrap(), vec![changed]);
    assert!(s.rule_update(999, &rule).await.unwrap().is_none());
    assert!(s.rule_delete(made.id).await.unwrap());
    assert!(s.rules_list().await.unwrap().is_empty());
}

#[tokio::test]
async fn only_the_latest_200_deliveries_are_kept() {
    let s = Store::open_in_memory().await.unwrap();
    for i in 0..205 {
        s.delivery_record("ops", &format!("check {i}"), "down", i % 2 == 0, 1, None)
            .await
            .unwrap();
    }
    let kept = s.deliveries_recent(500).await.unwrap();
    assert_eq!(kept.len(), 200);
    assert_eq!(kept[0].subject, "check 204", "newest first");
    assert_eq!(kept[199].subject, "check 5");
}

#[tokio::test]
async fn runs_are_kept_and_read_as_uptime_and_a_series() {
    let m = MetricsStore::open_in_memory().await.unwrap();
    // On the hour.
    let base = 1_800_000_000;
    for i in 0..60 {
        let ok = i % 4 != 0;
        m.record_check(
            7,
            base + i * 60,
            ok,
            ok.then_some(100 + u32::try_from(i).unwrap()),
        )
        .await
        .unwrap();
    }
    m.record_check(8, base, true, Some(5)).await.unwrap();

    let uptime = m.check_uptime(base).await.unwrap();
    assert_eq!(uptime.get(&7), Some(&(45, 60)));
    assert_eq!(uptime.get(&8), Some(&(1, 1)));

    let recent = m.check_recent(5, base).await.unwrap();
    let seven = &recent[&7];
    assert_eq!(seven.len(), 5);
    assert_eq!(seven.last(), Some(&Some(159)), "oldest first, newest last");
    assert_eq!(seven[1], None, "a failed run has no latency");

    let points = m
        .check_series(7, Resolution::Minute, base, base + 3_600, 6)
        .await
        .unwrap();
    assert_eq!(points.len(), 6);
    assert_eq!(points.iter().map(|p| p.total).sum::<u32>(), 60);
    assert_eq!(points.iter().map(|p| p.up).sum::<u32>(), 45);

    // An hour folded, then read at the hour.
    m.rollup_checks(base, base + 3_600).await.unwrap();
    let hourly = m.check_uptime_hourly(base).await.unwrap();
    assert_eq!(hourly.get(&7), Some(&(45, 60)));
    let hours = m
        .check_series(7, Resolution::Quarter, base, base + 3_600, 300)
        .await
        .unwrap();
    assert_eq!(hours.len(), 1);
    assert!(hours[0].latency_max.unwrap() >= 159.0);

    m.forget_check(7).await.unwrap();
    assert!(!m.check_uptime(base).await.unwrap().contains_key(&7));
    assert!(!m.check_uptime_hourly(base).await.unwrap().contains_key(&7));
}

#[tokio::test]
async fn runs_are_pruned_after_a_week_and_hours_after_a_year() {
    let m = MetricsStore::open_in_memory().await.unwrap();
    let now = 1_800_000_000;
    m.record_check(1, now - 8 * 86_400, true, Some(1))
        .await
        .unwrap();
    m.record_check(1, now - 86_400, true, Some(1))
        .await
        .unwrap();
    m.rollup_checks(now - 9 * 86_400, now).await.unwrap();
    m.prune(now).await.unwrap();
    assert_eq!(m.check_uptime(0).await.unwrap().get(&1), Some(&(1, 1)));
    assert_eq!(
        m.check_uptime_hourly(0).await.unwrap().get(&1),
        Some(&(2, 2))
    );
    m.prune(now + 366 * 86_400).await.unwrap();
    assert!(m.check_uptime_hourly(0).await.unwrap().is_empty());
}
