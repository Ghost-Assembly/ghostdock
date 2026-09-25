//! Uptime checks: what is watched, how it is doing, and what happened.
//!
//! A check runs from GhostDock's own network: an HTTP(S) request, a TCP
//! connection, or a container's own running and health state as Docker
//! reports it.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Checks run no more often than this, in seconds.
pub const MIN_INTERVAL_S: u32 = 20;
pub const DEFAULT_INTERVAL_S: u32 = 60;
pub const DEFAULT_TIMEOUT_S: u32 = 10;
/// Consecutive failures before a check is down.
pub const DEFAULT_RETRIES: u32 = 2;
/// A certificate this close to expiring degrades its check.
pub const TLS_WARN_DAYS: i64 = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    /// A request to an `http` or `https` URL.
    Http,
    /// A connection to `host:port`.
    Tcp,
    /// A container's running and health state, by its name.
    Container,
}

impl CheckKind {
    pub const ALL: [Self; 3] = [Self::Http, Self::Tcp, Self::Container];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Tcp => "tcp",
            Self::Container => "container",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    /// Not yet run since it was made, changed or GhostDock started.
    Pending,
    Up,
    /// Answering, but its certificate is close to expiring or it is slow.
    Degraded,
    /// Failed as many times in a row as it is allowed to.
    Down,
    /// Turned off.
    Paused,
}

impl CheckState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Up => "up",
            Self::Degraded => "degraded",
            Self::Down => "down",
            Self::Paused => "paused",
        }
    }
}

/// A check as it is set up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub id: i64,
    pub name: String,
    pub kind: CheckKind,
    /// A URL, `host:port`, or a container name.
    pub target: String,
    pub interval_s: u32,
    pub timeout_s: u32,
    pub retries: u32,
    /// The statuses an HTTP check accepts, inclusive.
    pub expect_status_min: u16,
    pub expect_status_max: u16,
    /// Text an HTTP check's body must contain, within its first MiB.
    pub keyword: Option<String>,
    /// Slower than this is degraded.
    pub latency_warn_ms: Option<u32>,
    /// The stack it belongs to, shown on that stack's page and board row.
    pub stack_id: Option<i64>,
    pub enabled: bool,
    /// Whether its changes are sent to the alert channels.
    pub notify: bool,
    pub created_at: DateTime<Utc>,
}

/// A check's settings, as sent. Creating one needs a name, a kind and a
/// target, and takes defaults for the rest; a change sends only what
/// changes. An empty keyword, a latency limit of 0 and a stack id of 0 each
/// remove what was set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<CheckKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_s: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect_status_min: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect_status_max: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_warn_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<bool>,
}

/// How a check is doing now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckStatus {
    pub check_id: i64,
    pub state: CheckState,
    /// When it entered this state, as far as this run of GhostDock knows.
    pub since: Option<DateTime<Utc>>,
    /// When it last ran.
    pub last_at: Option<DateTime<Utc>>,
    pub latency_ms: Option<u32>,
    /// Why it is down or degraded, in words.
    pub message: Option<String>,
    /// Days until an HTTPS check's certificate expires.
    pub tls_days_left: Option<i64>,
}

/// A check with how it is doing and has done, for a list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckSummary {
    pub check: Check,
    pub status: CheckStatus,
    /// Share of runs that succeeded, 0 to 1; `None` before any ran.
    pub uptime_24h: Option<f64>,
    pub uptime_30d: Option<f64>,
    /// The latest runs' latencies, oldest first; `None` where one failed.
    pub recent: Vec<Option<u32>>,
}

/// A stretch of time a check was down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Incident {
    pub id: i64,
    pub check_id: i64,
    pub check_name: String,
    pub started_at: DateTime<Utc>,
    /// `None` while it is still down.
    pub ended_at: Option<DateTime<Utc>>,
    pub cause: String,
}

/// Runs folded into one point of a chart.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CheckPoint {
    /// Unix seconds at the start of the point.
    pub t: i64,
    pub up: u32,
    pub total: u32,
    pub latency_avg: Option<f64>,
    pub latency_max: Option<f64>,
}

/// A check's history over a range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckHistory {
    pub check_id: i64,
    pub range: crate::metrics::Range,
    /// Seconds each point stands for.
    pub step: i64,
    pub points: Vec<CheckPoint>,
    pub uptime: Option<f64>,
    /// Newest first, within the range.
    pub incidents: Vec<Incident>,
}

/// A share as a person reads it: "99.95%", "100%", or "no runs yet".
#[must_use]
pub fn format_uptime(share: Option<f64>) -> String {
    match share {
        None => "no runs yet".to_owned(),
        Some(s) if s >= 1.0 => "100%".to_owned(),
        // Never rounded up to 100%: one failure in thousands still shows.
        Some(s) => {
            let percent = (s * 10_000.0).floor() / 100.0;
            format!("{percent:.2}%")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_name_themselves_the_same_way_everywhere() {
        for kind in CheckKind::ALL {
            assert_eq!(CheckKind::parse(kind.as_str()), Some(kind));
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::Value::String(kind.as_str().to_owned())
            );
        }
        assert_eq!(CheckKind::parse("icmp"), None);
    }

    #[test]
    fn uptime_is_never_rounded_up_to_perfect() {
        assert_eq!(format_uptime(None), "no runs yet");
        assert_eq!(format_uptime(Some(1.0)), "100%");
        assert_eq!(format_uptime(Some(0.99999)), "99.99%");
        assert_eq!(format_uptime(Some(0.5)), "50.00%");
        assert_eq!(format_uptime(Some(0.0)), "0.00%");
    }

    #[test]
    fn input_sends_only_what_was_given() {
        let input: CheckInput = serde_json::from_str(
            r#"{"name":"web","kind":"http","target":"https://example.test/","retries":3}"#,
        )
        .unwrap();
        assert_eq!(input.kind, Some(CheckKind::Http));
        assert_eq!(input.retries, Some(3));
        assert_eq!(input.interval_s, None);
        let change = CheckInput {
            retries: Some(1),
            ..CheckInput::default()
        };
        assert_eq!(serde_json::to_string(&change).unwrap(), r#"{"retries":1}"#);
    }
}
