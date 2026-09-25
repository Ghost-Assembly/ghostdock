//! Uptime checks: what a check may be set to, how one run's result is
//! judged, and how results move a check between states.
//!
//! A check starts pending. The first success makes it up; it goes down
//! only after `retries` failures in a row, so one lost packet is not an
//! outage. Up with a reason (a certificate close to expiring, an answer
//! slower than its limit) is degraded. Going down opens an incident and
//! coming back closes it; an incident left open when GhostDock stopped is
//! carried over rather than opened again.

use shared::checks::{
    Check, CheckInput, CheckKind, CheckState, DEFAULT_INTERVAL_S, DEFAULT_RETRIES,
    DEFAULT_TIMEOUT_S, MIN_INTERVAL_S, TLS_WARN_DAYS,
};
use shared::container::{ContainerState, Health};

/// A day, the longest a check may wait between runs.
const MAX_INTERVAL_S: u32 = 86_400;
const MAX_TIMEOUT_S: u32 = 60;
const MAX_RETRIES: u32 = 10;
const MAX_NAME: usize = 100;
const MAX_TARGET: usize = 2_048;
const MAX_KEYWORD: usize = 256;
const MAX_LATENCY_MS: u32 = 60_000;

/// What one run found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    Ok,
    /// Answered, with something worth knowing about.
    Degraded(String),
    Failed(String),
}

/// A check's state between runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tracker {
    pub state: CheckState,
    /// Failures in a row.
    pub failures: u32,
    /// Whether an incident is open, which may be from before a restart.
    pub incident_open: bool,
}

/// What one run changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Step {
    /// From and to, when the state moved.
    pub changed: Option<(CheckState, CheckState)>,
    /// Open an incident, for this cause.
    pub open: Option<String>,
    /// Close the open incident.
    pub close: bool,
    /// Tell the alert channels.
    pub alert: bool,
}

impl Tracker {
    #[must_use]
    pub fn new(incident_open: bool) -> Self {
        Self {
            state: CheckState::Pending,
            failures: 0,
            incident_open,
        }
    }

    /// Folds in one run's result. `retries` is how many failures in a row
    /// make it down; fewer than one is taken as one.
    pub fn observe(&mut self, probe: &Probe, retries: u32) -> Step {
        let before = self.state;
        let mut step = Step::default();
        match probe {
            Probe::Ok | Probe::Degraded(_) => {
                self.failures = 0;
                self.state = if matches!(probe, Probe::Ok) {
                    CheckState::Up
                } else {
                    CheckState::Degraded
                };
                if self.incident_open {
                    self.incident_open = false;
                    step.close = true;
                }
            }
            Probe::Failed(cause) => {
                self.failures = self.failures.saturating_add(1);
                if self.failures >= retries.max(1) {
                    self.state = CheckState::Down;
                    if !self.incident_open {
                        self.incident_open = true;
                        step.open = Some(cause.clone());
                    }
                }
            }
        }
        if before != self.state {
            step.changed = Some((before, self.state));
            // Starting up is not news; an outage opening or closing is,
            // even across a restart.
            step.alert = before != CheckState::Pending || step.open.is_some() || step.close;
        }
        step
    }
}

/// Judges a run that got an answer: an expired certificate is a failure,
/// one close to expiring or a slow answer is degraded.
#[must_use]
pub fn assess(
    tls_days_left: Option<i64>,
    latency_ms: Option<u32>,
    latency_warn_ms: Option<u32>,
) -> Probe {
    if let Some(days) = tls_days_left {
        if days < 0 {
            return Probe::Failed("its certificate has expired".to_owned());
        }
        if days <= TLS_WARN_DAYS {
            return Probe::Degraded(format!("its certificate expires in {days} days"));
        }
    }
    if let (Some(took), Some(limit)) = (latency_ms, latency_warn_ms)
        && took > limit
    {
        return Probe::Degraded(format!("slow: {took} ms, over {limit} ms"));
    }
    Probe::Ok
}

/// Whether an HTTP answer is the one wanted: a status in range, and the
/// keyword where there is one (`None` when none was asked for).
pub fn judge_http(
    status: u16,
    expect: (u16, u16),
    keyword_found: Option<bool>,
) -> Result<(), String> {
    if status < expect.0 || status > expect.1 {
        return Err(format!("answered {status}, not {}–{}", expect.0, expect.1));
    }
    if keyword_found == Some(false) {
        return Err("the keyword is not in the page".to_owned());
    }
    Ok(())
}

/// Judges a container by what Docker says of it: running and not
/// unhealthy is up. `None` when no container has the name.
#[must_use]
pub fn judge_container(found: Option<(ContainerState, Option<Health>)>) -> Probe {
    match found {
        None => Probe::Failed("no container has that name".to_owned()),
        Some((_, Some(Health::Unhealthy))) => {
            Probe::Failed("its health check is failing".to_owned())
        }
        Some((state, _)) if state.is_running() => Probe::Ok,
        Some((state, _)) => Probe::Failed(format!(
            "it is {}",
            match state {
                ContainerState::Created => "created, not started",
                ContainerState::Paused => "paused",
                ContainerState::Removing => "being removed",
                ContainerState::Dead => "dead",
                _ => "stopped",
            }
        )),
    }
}

/// The check `input` makes, from `current` when changing one or from the
/// defaults when making one, if it is one GhostDock can run.
pub fn settle(input: &CheckInput, current: Option<&Check>) -> Result<Check, String> {
    let pick = |given: Option<&str>, had: Option<&str>| {
        given.or(had).map(str::trim).unwrap_or_default().to_owned()
    };
    let name = pick(input.name.as_deref(), current.map(|c| c.name.as_str()));
    if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(format!(
            "Give the check a name of up to {MAX_NAME} characters."
        ));
    }
    let kind = input
        .kind
        .or(current.map(|c| c.kind))
        .ok_or("Choose a kind: http, tcp or container.")?;
    let target = pick(input.target.as_deref(), current.map(|c| c.target.as_str()));
    if target.is_empty() {
        return Err("Give the check a target to reach.".to_owned());
    }
    check_target(kind, &target)?;

    let interval_s = input
        .interval_s
        .or(current.map(|c| c.interval_s))
        .unwrap_or(DEFAULT_INTERVAL_S);
    if !(MIN_INTERVAL_S..=MAX_INTERVAL_S).contains(&interval_s) {
        return Err(format!(
            "Checks run at most every {MIN_INTERVAL_S} seconds and at least once a day."
        ));
    }
    let timeout_s = input
        .timeout_s
        .or(current.map(|c| c.timeout_s))
        .unwrap_or(DEFAULT_TIMEOUT_S);
    if timeout_s == 0 || timeout_s > MAX_TIMEOUT_S || timeout_s > interval_s {
        return Err(format!(
            "The timeout is 1 to {MAX_TIMEOUT_S} seconds, and no longer than the interval."
        ));
    }
    let retries = input
        .retries
        .or(current.map(|c| c.retries))
        .unwrap_or(DEFAULT_RETRIES);
    if retries == 0 || retries > MAX_RETRIES {
        return Err(format!(
            "A check goes down after 1 to {MAX_RETRIES} failures in a row."
        ));
    }
    let expect_status_min = input
        .expect_status_min
        .or(current.map(|c| c.expect_status_min))
        .unwrap_or(200);
    let expect_status_max = input
        .expect_status_max
        .or(current.map(|c| c.expect_status_max))
        .unwrap_or(399);
    if expect_status_min < 100 || expect_status_max > 599 || expect_status_min > expect_status_max {
        return Err("The accepted status range runs from 100 to 599, lowest first.".to_owned());
    }
    // Empty and zero clear what was set; absent keeps it.
    let keyword = match &input.keyword {
        Some(k) if k.is_empty() => None,
        Some(k) => Some(k.clone()),
        None => current.and_then(|c| c.keyword.clone()),
    };
    if keyword
        .as_ref()
        .is_some_and(|k| k.len() > MAX_KEYWORD || k.chars().any(char::is_control))
    {
        return Err(format!(
            "A keyword is one line of up to {MAX_KEYWORD} bytes."
        ));
    }
    let latency_warn_ms = match input.latency_warn_ms {
        Some(0) => None,
        Some(ms) => Some(ms),
        None => current.and_then(|c| c.latency_warn_ms),
    };
    if latency_warn_ms.is_some_and(|ms| ms > MAX_LATENCY_MS) {
        return Err(format!("The latency limit is at most {MAX_LATENCY_MS} ms."));
    }
    let stack_id = match input.stack_id {
        Some(0) => None,
        Some(id) => Some(id),
        None => current.and_then(|c| c.stack_id),
    };

    Ok(Check {
        id: current.map_or(0, |c| c.id),
        name,
        kind,
        target,
        interval_s,
        timeout_s,
        retries,
        expect_status_min,
        expect_status_max,
        keyword,
        latency_warn_ms,
        stack_id,
        enabled: input.enabled.or(current.map(|c| c.enabled)).unwrap_or(true),
        notify: input.notify.or(current.map(|c| c.notify)).unwrap_or(true),
        created_at: current.map(|c| c.created_at).unwrap_or_default(),
    })
}

/// Whether `target` is what a check of `kind` can reach.
pub fn check_target(kind: CheckKind, target: &str) -> Result<(), String> {
    if target.len() > MAX_TARGET || target.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("A target is one line with no spaces.".to_owned());
    }
    match kind {
        CheckKind::Http => check_url(target),
        CheckKind::Tcp => {
            let port = target
                .rsplit_once(':')
                .filter(|(host, _)| !host.is_empty() && !host.contains('/'))
                .and_then(|(_, port)| port.parse::<u16>().ok())
                .filter(|p| *p > 0);
            match port {
                Some(_) => Ok(()),
                None => Err("A TCP target is host:port, such as 127.0.0.1:5432.".to_owned()),
            }
        }
        CheckKind::Container => {
            if crate::logs::is_container_name(target) {
                Ok(())
            } else {
                Err("A container target is a container's name.".to_owned())
            }
        }
    }
}

/// Whether `url` is an `http` or `https` URL with a host.
pub fn check_url(url: &str) -> Result<(), String> {
    let fine = (url.starts_with("http://") || url.starts_with("https://"))
        && url_host(url).is_some()
        && !url.chars().any(|c| c.is_whitespace() || c.is_control());
    if fine {
        Ok(())
    } else {
        Err("Use an http:// or https:// address.".to_owned())
    }
}

/// The host part of a URL, and its port if it names one: never the path,
/// query or credentials, which is where secrets in a URL live.
#[must_use]
pub fn url_host(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    (!host.is_empty() && !host.starts_with(':') && !host.chars().any(char::is_whitespace))
        .then_some(host)
}
