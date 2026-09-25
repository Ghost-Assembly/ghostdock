//! Alerts: what a resource rule watches and when it fires, and the words
//! and shape of what is sent.

use chrono::{DateTime, Utc};
use shared::alerts::{Alert, AlertKind, Metric, NewAlertChannel, NewAlertRule};
use shared::checks::{Check, CheckKind, CheckState, CheckStatus};
use shared::metrics::{Reading, SubjectKind};

/// A day of minutes: the longest a rule may wait before firing.
const MAX_FOR_MIN: u32 = 1_440;
const MAX_NAME: usize = 100;

/// What a resource rule watches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    Host,
    Container(String),
    /// A registered stack, by id: its containers together.
    Stack(i64),
}

impl Subject {
    /// `host`, `container:<name>` or `stack:<id>`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        if text == "host" {
            return Some(Self::Host);
        }
        let (kind, rest) = text.split_once(':')?;
        match kind {
            "container" if crate::logs::is_container_name(rest) => {
                Some(Self::Container(rest.to_owned()))
            }
            "stack" => rest
                .parse::<i64>()
                .ok()
                .filter(|id| *id > 0)
                .map(Self::Stack),
            _ => None,
        }
    }
}

/// The subject a rule watches, if the rule is one GhostDock can evaluate.
pub fn check_rule(rule: &NewAlertRule) -> Result<Subject, String> {
    let subject = Subject::parse(rule.subject.trim())
        .ok_or("A rule watches host, container:<name> or stack:<id>.")?;
    if rule.metric == Metric::Disk && subject != Subject::Host {
        return Err("Disk space is the host's: watch it on host.".to_owned());
    }
    if !(rule.above_pct > 0.0 && rule.above_pct <= 100.0) {
        return Err("The threshold is a percentage above 0 and at most 100.".to_owned());
    }
    if rule.for_min == 0 || rule.for_min > MAX_FOR_MIN {
        return Err(format!(
            "A rule waits 1 to {MAX_FOR_MIN} minutes before alerting."
        ));
    }
    Ok(subject)
}

/// Whether a new channel is one GhostDock can send to.
pub fn check_channel(new: &NewAlertChannel) -> Result<(), String> {
    let name = new.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return Err(format!(
            "Give the channel a name of up to {MAX_NAME} characters."
        ));
    }
    crate::checks::check_url(new.url.trim())?;
    if new
        .token
        .as_ref()
        .is_some_and(|t| t.chars().any(|c| c.is_control() || c == ' '))
    {
        return Err("A token is one word with no spaces.".to_owned());
    }
    Ok(())
}

/// A rule's streak of minutes over its threshold, and whether it has
/// fired.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleTracker {
    streak: u32,
    pub firing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleChange {
    Fired,
    Resolved,
}

impl RuleTracker {
    /// Folds in one minute's figure. A minute with none (a stopped
    /// container, a daemon not answering) changes nothing either way.
    pub fn observe(&mut self, pct: Option<f64>, above: f64, for_min: u32) -> Option<RuleChange> {
        let pct = pct?;
        if pct > above {
            self.streak = self.streak.saturating_add(1);
            if !self.firing && self.streak >= for_min.max(1) {
                self.firing = true;
                return Some(RuleChange::Fired);
            }
        } else {
            self.streak = 0;
            if self.firing {
                self.firing = false;
                return Some(RuleChange::Resolved);
            }
        }
        None
    }
}

/// One subject's reading for a minute, as sampling filed it.
#[derive(Debug, Clone, Copy)]
pub struct Sample<'a> {
    pub kind: SubjectKind,
    pub key: &'a str,
    pub project: Option<&'a str>,
    pub reading: &'a Reading,
}

/// What a rule's subject covers once a stack's id is its project name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope<'a> {
    Host,
    Container(&'a str),
    Project(&'a str),
}

/// `metric` of `scope` as a percentage: CPU of the host's cores, memory of
/// a container's limit or else the host's memory, disk of the fullest
/// watched disk's capacity. `None` when nothing was measured.
#[allow(clippy::cast_precision_loss)]
#[must_use]
pub fn usage(
    scope: Scope<'_>,
    metric: Metric,
    samples: &[Sample<'_>],
    cpus: Option<u32>,
    host_memory: Option<u64>,
) -> Option<f64> {
    let share = |part: f64, whole: f64| (whole > 0.0).then(|| part / whole * 100.0);
    if metric == Metric::Disk {
        return samples
            .iter()
            .filter(|s| s.kind == SubjectKind::Disk && scope == Scope::Host)
            .filter_map(|s| share(s.reading.mem? as f64, s.reading.mem_limit? as f64))
            .reduce(f64::max);
    }
    let chosen: Vec<&Reading> = samples
        .iter()
        .filter(|s| match scope {
            Scope::Host => s.kind == SubjectKind::Host,
            Scope::Container(name) => s.kind == SubjectKind::Container && s.key == name,
            Scope::Project(p) => s.kind == SubjectKind::Container && s.project == Some(p),
        })
        .map(|s| s.reading)
        .collect();
    if chosen.is_empty() {
        return None;
    }
    match metric {
        Metric::Cpu => {
            let cores: f64 = chosen.iter().filter_map(|r| r.cpu).sum();
            share(cores, f64::from(cpus?))
        }
        _ => {
            let used: u64 = chosen.iter().filter_map(|r| r.mem).sum();
            let whole = match (scope, chosen.as_slice()) {
                // One container, or the host: its own limit or total.
                (Scope::Host | Scope::Container(_), [one]) => one.mem_limit.or(host_memory)?,
                _ => host_memory?,
            };
            share(used as f64, whole as f64)
        }
    }
}

/// The alert for a check that has just changed state.
#[must_use]
pub fn check_alert(check: &Check, status: &CheckStatus, at: DateTime<Utc>) -> Alert {
    let name = &check.name;
    let why = status
        .message
        .as_deref()
        .map(|m| format!(": {m}"))
        .unwrap_or_default();
    let message = match status.state {
        CheckState::Down => format!("{name} is down{why}"),
        CheckState::Degraded => format!("{name} is degraded{why}"),
        CheckState::Up => format!("{name} is up again"),
        CheckState::Pending | CheckState::Paused => format!("{name} is {}", status.state.as_str()),
    };
    Alert {
        kind: AlertKind::Check,
        subject: name.clone(),
        state: status.state.as_str().to_owned(),
        message,
        at,
        url: (check.kind == CheckKind::Http).then(|| check.target.clone()),
    }
}

/// The alert for a resource rule that has just fired or resolved.
#[must_use]
pub fn rule_alert(
    subject: &str,
    metric: Metric,
    above_pct: f64,
    for_min: u32,
    now_pct: Option<f64>,
    change: RuleChange,
    at: DateTime<Utc>,
) -> Alert {
    let now = now_pct
        .map(|p| format!(" (now {p:.0}%)"))
        .unwrap_or_default();
    let (state, message) = match change {
        RuleChange::Fired => (
            "firing",
            format!(
                "{subject}: {} above {above_pct:.0}% for {for_min} min{now}",
                metric.label()
            ),
        ),
        RuleChange::Resolved => (
            "resolved",
            format!(
                "{subject}: {} back under {above_pct:.0}%{now}",
                metric.label()
            ),
        ),
    };
    Alert {
        kind: AlertKind::Resource,
        subject: subject.to_owned(),
        state: state.to_owned(),
        message,
        at,
        url: None,
    }
}

/// An alert as ntfy takes it: the message as the body, and headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ntfy {
    pub title: String,
    pub priority: &'static str,
    pub tags: &'static str,
    pub body: String,
}

#[must_use]
pub fn ntfy(alert: &Alert) -> Ntfy {
    let (priority, tags) = match alert.state.as_str() {
        "down" | "firing" => ("high", "rotating_light"),
        "degraded" => ("default", "warning"),
        "test" => ("low", "bell"),
        _ => ("default", "white_check_mark"),
    };
    // A header carries visible ASCII only; the body says it in full.
    let title = format!("GhostDock: {} {}", alert.subject, alert.state)
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '?'
            }
        })
        .collect();
    Ntfy {
        title,
        priority,
        tags,
        body: alert.message.clone(),
    }
}
