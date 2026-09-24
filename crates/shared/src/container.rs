//! Container wire types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Lifecycle state, normalised from the daemon's string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerState {
    Created,
    Running,
    Paused,
    Restarting,
    Removing,
    Exited,
    Dead,
    /// The daemon reported something we do not recognise.
    Unknown,
}

impl ContainerState {
    #[must_use]
    pub fn is_running(self) -> bool {
        matches!(self, Self::Running | Self::Restarting)
    }
}

impl std::str::FromStr for ContainerState {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "created" => Self::Created,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "restarting" => Self::Restarting,
            "removing" => Self::Removing,
            "exited" => Self::Exited,
            "dead" => Self::Dead,
            _ => Self::Unknown,
        })
    }
}

/// Healthcheck result, where the image defines one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Starting,
    Healthy,
    Unhealthy,
}

/// A published port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    /// Absent for a port exposed but not published to the host.
    pub host_ip: Option<String>,
    pub host_port: Option<u16>,
    pub container_port: u16,
    /// `tcp` or `udp`.
    pub protocol: String,
}

/// Compose ownership, read from the `com.docker.compose.*` labels.
///
/// Absent for a container started outside Compose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposeMembership {
    pub project: String,
    pub service: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Container {
    pub id: String,
    /// Primary name, with the daemon's leading slash stripped.
    pub name: String,
    pub image: String,
    pub state: ContainerState,
    /// The daemon's human-readable status, e.g. `Up 3 days (healthy)`.
    pub status: String,
    pub health: Option<Health>,
    pub created: Option<DateTime<Utc>>,
    pub ports: Vec<Port>,
    pub compose: Option<ComposeMembership>,
}
