//! Alerts: where they go, what raises them, and what was sent.
//!
//! A channel's URL and token are write-only, like every other secret: they
//! go in when the channel is added and only the host the URL points at
//! comes back out. A token in a URL's path, as webhook URLs often carry,
//! stays hidden with it.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    /// A JSON POST: `{ kind, subject, state, message, at, url }`.
    Webhook,
    /// A POST of the message to an ntfy topic URL.
    Ntfy,
}

impl ChannelKind {
    pub const ALL: [Self; 2] = [Self::Webhook, Self::Ntfy];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webhook => "webhook",
            Self::Ntfy => "ntfy",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == text)
    }
}

/// A channel as listed: never its URL or token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertChannel {
    pub id: i64,
    pub name: String,
    pub kind: ChannelKind,
    /// The host the URL points at, and its port if it names one.
    pub host: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewAlertChannel {
    pub name: String,
    pub kind: ChannelKind,
    pub url: String,
    /// Sent as `Authorization: Bearer`, for an ntfy topic that needs one.
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    Cpu,
    Memory,
    /// The fullest watched disk; the host only.
    Disk,
}

impl Metric {
    pub const ALL: [Self; 3] = [Self::Cpu, Self::Memory, Self::Disk];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Memory => "memory",
            Self::Disk => "disk",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == text)
    }

    /// As a sentence says it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "memory",
            Self::Disk => "disk",
        }
    }
}

/// A resource rule: alert when `metric` of `subject` stays above
/// `above_pct` for `for_min` minutes, and again when it falls back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: i64,
    /// `host`, `container:<name>` or `stack:<id>`.
    pub subject: String,
    pub metric: Metric,
    /// Percent: of the host's cores for CPU, of the memory limit (or the
    /// host's memory) for memory, of capacity for a disk.
    pub above_pct: f64,
    pub for_min: u32,
    pub enabled: bool,
    /// Whether it is breached now and has been alerted.
    pub firing: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewAlertRule {
    pub subject: String,
    pub metric: Metric,
    pub above_pct: f64,
    pub for_min: u32,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// What raised an alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertKind {
    Check,
    Resource,
    Test,
}

/// One alert, as a webhook receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alert {
    pub kind: AlertKind,
    /// The check's name, or what a resource rule watches.
    pub subject: String,
    /// `down`, `up`, `degraded`, `firing`, `resolved` or `test`.
    pub state: String,
    pub message: String,
    pub at: DateTime<Utc>,
    /// What an HTTP check requests, where there is one.
    pub url: Option<String>,
}

/// One attempt to tell a channel something, and how it went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertDelivery {
    pub id: i64,
    /// The channel's name at the time, kept after it is removed.
    pub channel: String,
    pub subject: String,
    pub state: String,
    pub at: DateTime<Utc>,
    pub ok: bool,
    pub attempts: u32,
    /// Why it failed, without the URL.
    pub error: Option<String>,
}
