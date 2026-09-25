//! Uptime checks: how a run's result moves a check between states, and
//! what a check may be set to.

use domain::checks::{Probe, Tracker, assess, judge_http, settle, url_host};
use shared::checks::{Check, CheckInput, CheckKind, CheckState};

fn failed() -> Probe {
    Probe::Failed("connection refused".to_owned())
}

#[test]
fn a_new_check_is_pending_until_it_runs() {
    assert_eq!(Tracker::new(false).state, CheckState::Pending);
}

#[test]
fn it_is_up_on_the_first_success() {
    let mut t = Tracker::new(false);
    let step = t.observe(&Probe::Ok, 2);
    assert_eq!(t.state, CheckState::Up);
    assert_eq!(step.changed, Some((CheckState::Pending, CheckState::Up)));
    assert!(!step.alert, "a check starting up is not news");
    assert_eq!(step.open, None);
    assert!(!step.close);
}

#[test]
fn it_is_down_only_after_as_many_failures_in_a_row_as_allowed() {
    let mut t = Tracker::new(false);
    t.observe(&Probe::Ok, 3);
    for _ in 0..2 {
        let step = t.observe(&failed(), 3);
        assert_eq!(t.state, CheckState::Up, "one blip is not an outage");
        assert_eq!(step.changed, None);
        assert!(!step.alert);
    }
    let step = t.observe(&failed(), 3);
    assert_eq!(t.state, CheckState::Down);
    assert_eq!(step.changed, Some((CheckState::Up, CheckState::Down)));
    assert_eq!(step.open.as_deref(), Some("connection refused"));
    assert!(step.alert);

    // Still down: no second incident, no second alert.
    let step = t.observe(&failed(), 3);
    assert_eq!(step, domain::checks::Step::default());
}

#[test]
fn a_success_between_failures_starts_the_count_again() {
    let mut t = Tracker::new(false);
    t.observe(&failed(), 2);
    t.observe(&Probe::Ok, 2);
    t.observe(&failed(), 2);
    assert_eq!(t.state, CheckState::Up);
    t.observe(&failed(), 2);
    assert_eq!(t.state, CheckState::Down);
}

#[test]
fn coming_back_closes_the_incident_and_says_so() {
    let mut t = Tracker::new(false);
    t.observe(&failed(), 1);
    assert_eq!(t.state, CheckState::Down);
    let step = t.observe(&Probe::Ok, 1);
    assert_eq!(t.state, CheckState::Up);
    assert!(step.close);
    assert!(step.alert);
    assert_eq!(step.changed, Some((CheckState::Down, CheckState::Up)));
}

#[test]
fn zero_retries_still_needs_one_failure() {
    let mut t = Tracker::new(false);
    t.observe(&failed(), 0);
    assert_eq!(t.state, CheckState::Down);
}

#[test]
fn degraded_is_up_with_a_reason_and_opens_no_incident() {
    let mut t = Tracker::new(false);
    t.observe(&Probe::Ok, 2);
    let step = t.observe(&Probe::Degraded("slow".to_owned()), 2);
    assert_eq!(t.state, CheckState::Degraded);
    assert_eq!(step.open, None);
    assert!(step.alert);
    let step = t.observe(&Probe::Ok, 2);
    assert_eq!(step.changed, Some((CheckState::Degraded, CheckState::Up)));
    assert!(!step.close);
}

#[test]
fn restarting_while_down_neither_opens_nor_alerts_again() {
    // GhostDock restarted with an incident still open: the check starts
    // pending, and finding it still down is not a new outage.
    let mut t = Tracker::new(true);
    let step = t.observe(&failed(), 1);
    assert_eq!(t.state, CheckState::Down);
    assert_eq!(step.open, None);
    assert!(!step.alert);
}

#[test]
fn restarting_while_down_and_finding_it_up_closes_the_incident() {
    let mut t = Tracker::new(true);
    let step = t.observe(&Probe::Ok, 2);
    assert_eq!(t.state, CheckState::Up);
    assert!(step.close);
    assert!(step.alert, "it came back while GhostDock was away");
}

#[test]
fn a_certificate_near_expiry_or_a_slow_answer_degrades() {
    assert_eq!(assess(Some(90), Some(120), Some(500)), Probe::Ok);
    assert_eq!(assess(None, None, None), Probe::Ok);
    match assess(Some(14), Some(10), None) {
        Probe::Degraded(why) => assert!(why.contains("14 days"), "{why}"),
        other => panic!("{other:?}"),
    }
    match assess(Some(90), Some(900), Some(500)) {
        Probe::Degraded(why) => assert!(why.contains("900 ms"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(
        matches!(assess(Some(-2), None, None), Probe::Failed(_)),
        "an expired certificate is down"
    );
}

#[test]
fn an_http_answer_is_judged_by_its_status_and_keyword() {
    assert_eq!(judge_http(200, (200, 399), None), Ok(()));
    assert_eq!(judge_http(301, (200, 399), None), Ok(()));
    assert!(
        judge_http(503, (200, 399), None)
            .unwrap_err()
            .contains("503")
    );
    assert_eq!(judge_http(200, (200, 299), Some(true)), Ok(()));
    assert!(judge_http(200, (200, 299), Some(false)).is_err());
}

#[test]
fn a_container_is_up_while_running_and_not_unhealthy() {
    use domain::checks::judge_container;
    use shared::container::{ContainerState, Health};
    assert_eq!(
        judge_container(Some((ContainerState::Running, None))),
        Probe::Ok
    );
    assert_eq!(
        judge_container(Some((ContainerState::Running, Some(Health::Starting)))),
        Probe::Ok,
        "still starting is not a failure"
    );
    assert!(matches!(
        judge_container(Some((ContainerState::Running, Some(Health::Unhealthy)))),
        Probe::Failed(_)
    ));
    match judge_container(Some((ContainerState::Exited, None))) {
        Probe::Failed(why) => assert!(why.contains("stopped"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(judge_container(None), Probe::Failed(_)));
}

fn http(target: &str) -> CheckInput {
    CheckInput {
        name: Some("Blog".to_owned()),
        kind: Some(CheckKind::Http),
        target: Some(target.to_owned()),
        ..CheckInput::default()
    }
}

#[test]
fn a_new_check_takes_the_defaults() {
    let check = settle(&http("https://blog.example.test/health"), None).unwrap();
    assert_eq!(check.interval_s, 60);
    assert_eq!(check.timeout_s, 10);
    assert_eq!(check.retries, 2);
    assert_eq!(
        (check.expect_status_min, check.expect_status_max),
        (200, 399)
    );
    assert!(check.enabled && check.notify);
    assert_eq!(check.keyword, None);
}

#[test]
fn a_change_keeps_what_it_does_not_name() {
    let mut current: Check = settle(&http("https://blog.example.test/"), None).unwrap();
    current.id = 4;
    current.keyword = Some("ok".to_owned());
    current.stack_id = Some(3);
    let change = CheckInput {
        retries: Some(5),
        ..CheckInput::default()
    };
    let changed = settle(&change, Some(&current)).unwrap();
    assert_eq!(changed.id, 4);
    assert_eq!(changed.retries, 5);
    assert_eq!(changed.keyword.as_deref(), Some("ok"));
    assert_eq!(changed.stack_id, Some(3));

    let cleared = CheckInput {
        keyword: Some(String::new()),
        stack_id: Some(0),
        latency_warn_ms: Some(0),
        ..CheckInput::default()
    };
    let changed = settle(&cleared, Some(&current)).unwrap();
    assert_eq!(
        (changed.keyword, changed.stack_id, changed.latency_warn_ms),
        (None, None, None)
    );
}

#[test]
fn settings_outside_what_makes_sense_are_refused() {
    let refused = |input: CheckInput| settle(&input, None).unwrap_err();
    assert!(
        refused(CheckInput {
            interval_s: Some(5),
            ..http("https://a.test/")
        })
        .contains("20 seconds")
    );
    assert!(
        refused(CheckInput {
            timeout_s: Some(0),
            ..http("https://a.test/")
        })
        .contains("timeout")
    );
    assert!(
        refused(CheckInput {
            timeout_s: Some(90),
            interval_s: Some(60),
            ..http("https://a.test/")
        })
        .contains("timeout")
    );
    assert!(
        refused(CheckInput {
            retries: Some(50),
            ..http("https://a.test/")
        })
        .contains("failures")
    );
    assert!(
        refused(CheckInput {
            expect_status_min: Some(400),
            expect_status_max: Some(200),
            ..http("https://a.test/")
        })
        .contains("status")
    );
    assert!(
        refused(CheckInput {
            name: Some("  ".to_owned()),
            ..http("https://a.test/")
        })
        .contains("name")
    );
    assert!(
        refused(CheckInput {
            kind: None,
            ..http("https://a.test/")
        })
        .contains("kind")
    );
    assert!(
        refused(CheckInput {
            target: None,
            ..http("https://a.test/")
        })
        .contains("target")
    );
}

#[test]
fn targets_are_what_their_kind_needs() {
    let target = |kind, target: &str| {
        settle(
            &CheckInput {
                name: Some("x".to_owned()),
                kind: Some(kind),
                target: Some(target.to_owned()),
                ..CheckInput::default()
            },
            None,
        )
    };
    for ok in [
        "http://127.0.0.1:8080/api/v1/health",
        "https://blog.example.test",
        "https://[::1]:8443/x?y=1",
    ] {
        assert!(target(CheckKind::Http, ok).is_ok(), "{ok}");
    }
    for bad in [
        "ftp://a.test/",
        "https://",
        "https:// a.test/",
        "blog.example.test",
        "file:///etc/passwd",
        "https://a.test/\nx",
    ] {
        assert!(target(CheckKind::Http, bad).is_err(), "{bad}");
    }
    for ok in ["127.0.0.1:5432", "db.example.test:6379", "[::1]:22"] {
        assert!(target(CheckKind::Tcp, ok).is_ok(), "{ok}");
    }
    for bad in [
        "127.0.0.1",
        "db:0",
        "db:70000",
        ":80",
        "a b:80",
        "http://x:80",
    ] {
        assert!(target(CheckKind::Tcp, bad).is_err(), "{bad}");
    }
    assert!(target(CheckKind::Container, "blog-web-1").is_ok());
    assert!(target(CheckKind::Container, "../etc").is_err());
    assert!(target(CheckKind::Container, "").is_err());
}

#[test]
fn only_the_host_of_a_url_is_shown() {
    assert_eq!(
        url_host("https://hooks.example.test/services/T0/B1/secret"),
        Some("hooks.example.test")
    );
    assert_eq!(
        url_host("http://127.0.0.1:9000/hook?token=abc"),
        Some("127.0.0.1:9000")
    );
    // Credentials in the URL are the secret, not the host.
    assert_eq!(
        url_host("https://user:pass@ntfy.example.test/topic"),
        Some("ntfy.example.test")
    );
    assert_eq!(
        url_host("https://ntfy.example.test#frag"),
        Some("ntfy.example.test")
    );
    assert_eq!(url_host("not a url"), None);
    assert_eq!(url_host("https:///path"), None);
}
