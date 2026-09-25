//! Alerts: what a rule watches, when it fires, and what is sent.

use chrono::{TimeZone as _, Utc};
use domain::alerts::{
    RuleChange, RuleTracker, Sample, Scope, Subject, check_alert, check_channel, check_rule, ntfy,
    usage,
};
use shared::alerts::{Alert, AlertKind, ChannelKind, Metric, NewAlertChannel, NewAlertRule};
use shared::checks::{Check, CheckKind, CheckState, CheckStatus};
use shared::metrics::{Reading, SubjectKind};

fn rule(subject: &str, metric: Metric, above_pct: f64, for_min: u32) -> NewAlertRule {
    NewAlertRule {
        subject: subject.to_owned(),
        metric,
        above_pct,
        for_min,
        enabled: true,
    }
}

#[test]
fn subjects_are_the_host_a_container_or_a_stack() {
    assert_eq!(Subject::parse("host"), Some(Subject::Host));
    assert_eq!(
        Subject::parse("container:blog-web-1"),
        Some(Subject::Container("blog-web-1".to_owned()))
    );
    assert_eq!(Subject::parse("stack:3"), Some(Subject::Stack(3)));
    for bad in [
        "",
        "stack:blog",
        "container:",
        "container:a/b",
        "disk:x",
        "hostile",
    ] {
        assert_eq!(Subject::parse(bad), None, "{bad}");
    }
}

#[test]
fn a_rule_must_make_sense() {
    assert!(check_rule(&rule("host", Metric::Cpu, 90.0, 5)).is_ok());
    assert!(check_rule(&rule("stack:2", Metric::Memory, 80.0, 1)).is_ok());
    assert!(check_rule(&rule("host", Metric::Disk, 90.0, 5)).is_ok());
    assert!(
        check_rule(&rule("container:web", Metric::Disk, 90.0, 5)).is_err(),
        "disks are the host's"
    );
    assert!(check_rule(&rule("host", Metric::Cpu, 0.0, 5)).is_err());
    assert!(check_rule(&rule("host", Metric::Cpu, 101.0, 5)).is_err());
    assert!(check_rule(&rule("host", Metric::Cpu, f64::NAN, 5)).is_err());
    assert!(check_rule(&rule("host", Metric::Cpu, 90.0, 0)).is_err());
    assert!(check_rule(&rule("nowhere", Metric::Cpu, 90.0, 5)).is_err());
}

#[test]
fn a_channel_needs_a_name_and_a_web_address() {
    let channel = |url: &str, token: Option<&str>| NewAlertChannel {
        name: "phone".to_owned(),
        kind: ChannelKind::Ntfy,
        url: url.to_owned(),
        token: token.map(str::to_owned),
    };
    assert!(check_channel(&channel("https://ntfy.example.test/ops", None)).is_ok());
    assert!(check_channel(&channel("http://127.0.0.1:9000/hook", Some("tk_abc"))).is_ok());
    assert!(check_channel(&channel("ftp://ntfy.example.test/ops", None)).is_err());
    assert!(
        check_channel(&channel("https://ntfy.example.test/ops", Some("a\nb"))).is_err(),
        "a header cannot hold a newline"
    );
    let mut nameless = channel("https://ntfy.example.test/ops", None);
    nameless.name = " ".to_owned();
    assert!(check_channel(&nameless).is_err());
}

#[test]
fn a_rule_fires_once_after_its_minutes_and_resolves_once() {
    let mut t = RuleTracker::default();
    assert_eq!(t.observe(Some(95.0), 90.0, 3), None);
    assert_eq!(t.observe(Some(95.0), 90.0, 3), None);
    assert_eq!(t.observe(Some(95.0), 90.0, 3), Some(RuleChange::Fired));
    assert!(t.firing);
    assert_eq!(
        t.observe(Some(97.0), 90.0, 3),
        None,
        "once, not every minute"
    );
    assert_eq!(t.observe(Some(50.0), 90.0, 3), Some(RuleChange::Resolved));
    assert!(!t.firing);
    assert_eq!(t.observe(Some(50.0), 90.0, 3), None);
}

#[test]
fn a_dip_starts_the_count_again_and_a_gap_changes_nothing() {
    let mut t = RuleTracker::default();
    t.observe(Some(95.0), 90.0, 2);
    t.observe(Some(10.0), 90.0, 2);
    assert_eq!(t.observe(Some(95.0), 90.0, 2), None);
    assert_eq!(
        t.observe(None, 90.0, 2),
        None,
        "no figures is not a reading"
    );
    assert_eq!(t.observe(Some(95.0), 90.0, 2), Some(RuleChange::Fired));
    assert_eq!(
        t.observe(None, 90.0, 2),
        None,
        "a stopped container does not resolve"
    );
}

fn reading(cpu: f64, mem: u64, limit: Option<u64>) -> Reading {
    Reading {
        cpu: Some(cpu),
        mem: Some(mem),
        mem_limit: limit,
        ..Reading::default()
    }
}

#[test]
fn usage_is_a_share_of_what_there_is() {
    let host = reading(2.0, 4 << 30, Some(16 << 30));
    let web = reading(1.0, 512 << 20, Some(1 << 30));
    let db = reading(0.5, 1 << 30, None);
    let disk = Reading {
        mem: Some(95),
        mem_limit: Some(100),
        ..Reading::default()
    };
    let small = Reading {
        mem: Some(10),
        mem_limit: Some(100),
        ..Reading::default()
    };
    let samples = [
        Sample {
            kind: SubjectKind::Host,
            key: "host",
            project: None,
            reading: &host,
        },
        Sample {
            kind: SubjectKind::Container,
            key: "blog-web-1",
            project: Some("blog"),
            reading: &web,
        },
        Sample {
            kind: SubjectKind::Container,
            key: "blog-db-1",
            project: Some("blog"),
            reading: &db,
        },
        Sample {
            kind: SubjectKind::Disk,
            key: "docker",
            project: None,
            reading: &disk,
        },
        Sample {
            kind: SubjectKind::Disk,
            key: "data",
            project: None,
            reading: &small,
        },
    ];
    let at = |scope, metric| usage(scope, metric, &samples, Some(8), Some(16 << 30));
    let close = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() < 1e-9);

    assert!(close(at(Scope::Host, Metric::Cpu), 25.0), "2 of 8 cores");
    assert!(close(at(Scope::Host, Metric::Memory), 25.0));
    assert!(
        close(at(Scope::Host, Metric::Disk), 95.0),
        "the fullest disk"
    );
    assert!(
        close(at(Scope::Container("blog-web-1"), Metric::Memory), 50.0),
        "of its own limit"
    );
    assert!(
        close(at(Scope::Container("blog-db-1"), Metric::Memory), 6.25),
        "no limit: of the host"
    );
    assert!(close(at(Scope::Container("blog-web-1"), Metric::Cpu), 12.5));
    assert!(close(at(Scope::Project("blog"), Metric::Cpu), 18.75));
    assert_eq!(
        at(Scope::Container("gone-1"), Metric::Cpu),
        None,
        "not running, no reading"
    );
    assert_eq!(at(Scope::Project("gone"), Metric::Memory), None);
    assert_eq!(
        usage(Scope::Host, Metric::Cpu, &samples, None, None),
        None,
        "no core count, no share"
    );
}

fn check() -> Check {
    Check {
        id: 1,
        name: "Blog".to_owned(),
        kind: CheckKind::Http,
        target: "https://blog.example.test/".to_owned(),
        interval_s: 60,
        timeout_s: 10,
        retries: 2,
        expect_status_min: 200,
        expect_status_max: 399,
        keyword: None,
        latency_warn_ms: None,
        stack_id: None,
        enabled: true,
        notify: true,
        created_at: Utc::now(),
    }
}

#[test]
fn a_check_alert_says_what_happened_and_why() {
    let at = Utc.with_ymd_and_hms(2026, 9, 25, 12, 0, 0).unwrap();
    let status = CheckStatus {
        check_id: 1,
        state: CheckState::Down,
        since: Some(at),
        last_at: Some(at),
        latency_ms: None,
        message: Some("connection refused".to_owned()),
        tls_days_left: None,
    };
    let alert = check_alert(&check(), &status, at);
    assert_eq!(alert.kind, AlertKind::Check);
    assert_eq!(alert.subject, "Blog");
    assert_eq!(alert.state, "down");
    assert!(
        alert.message.contains("connection refused"),
        "{}",
        alert.message
    );
    assert_eq!(alert.url.as_deref(), Some("https://blog.example.test/"));

    let mut tcp = check();
    tcp.kind = CheckKind::Tcp;
    tcp.target = "db.example.test:5432".to_owned();
    assert_eq!(
        check_alert(&tcp, &status, at).url,
        None,
        "only an HTTP check has a URL"
    );
}

#[test]
fn ntfy_gets_a_title_a_priority_and_tags() {
    let alert = |state: &str| Alert {
        kind: AlertKind::Check,
        subject: "Blog".to_owned(),
        state: state.to_owned(),
        message: "Blog is down: connection refused".to_owned(),
        at: Utc::now(),
        url: None,
    };
    let down = ntfy(&alert("down"));
    assert!(down.title.contains("Blog"));
    assert_eq!(down.priority, "high");
    assert_eq!(down.body, "Blog is down: connection refused");
    let up = ntfy(&alert("up"));
    assert_eq!(up.priority, "default");
    assert_ne!(down.tags, up.tags);
}
