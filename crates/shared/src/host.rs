//! Docker host wire types.
//!
//! v1 manages exactly one host, but every resource is addressed beneath a
//! host id so that adding more later is a feature rather than a migration.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub id: i64,
    /// Display name, e.g. `local`.
    pub name: String,
}

/// Daemon facts, shown on the host overview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    pub id: i64,
    pub name: String,
    /// Daemon version string, or `None` when the daemon is unreachable.
    pub server_version: Option<String>,
    pub containers_running: u64,
    pub containers_total: u64,
    pub images: u64,
    /// Populated when the daemon could not be reached.
    pub unreachable_reason: Option<String>,
    /// Problems with how GhostDock itself is deployed, in words.
    ///
    /// Shown prominently because the ones found here fail silently otherwise:
    /// a deploy reports success while the application reads nothing.
    #[serde(default)]
    pub problems: Vec<String>,
}
