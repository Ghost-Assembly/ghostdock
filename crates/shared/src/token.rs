//! API tokens and the permissions they carry.
//!
//! A person signed in through the browser can do everything. A token can do
//! only what it was granted, one action at a time, so a client such as an AI
//! assistant can be given exactly as much of the host as it needs: allowed to
//! restart but not to deploy, say, or to read logs but not compose files.
//! Managing accounts and tokens is never grantable: a token cannot mint a
//! stronger token or an account to sign in as.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Permission {
    /// Stacks, containers, images, resource figures, history, update status,
    /// uptime checks and alert settings. Names, never secrets.
    #[serde(rename = "host.view")]
    HostView,
    /// Compose files as registered. Inline files can hold pasted values.
    #[serde(rename = "stacks.read_compose")]
    ComposeRead,
    /// Container output, which often contains things worth keeping private.
    #[serde(rename = "logs.view")]
    LogsView,
    /// The audit trail.
    #[serde(rename = "activity.view")]
    ActivityView,
    /// `docker compose up`: pulls images and applies changes, including updates.
    #[serde(rename = "stacks.deploy")]
    StacksDeploy,
    /// Restart containers in place.
    #[serde(rename = "stacks.restart")]
    StacksRestart,
    /// Stop containers, leaving them to be started again.
    #[serde(rename = "stacks.stop")]
    StacksStop,
    /// `docker compose down`: containers and networks go; volumes and the registration stay.
    #[serde(rename = "stacks.take_down")]
    StacksTakeDown,
    /// Check for new commits and image digests now rather than on schedule.
    #[serde(rename = "updates.check")]
    UpdatesCheck,
    /// Turn automatic deploys of updates on or off.
    #[serde(rename = "updates.auto_apply")]
    UpdatesAutoApply,
    /// Register stacks, inline or from a repository.
    #[serde(rename = "stacks.create")]
    StacksCreate,
    /// Change a registered stack's compose file.
    #[serde(rename = "stacks.edit")]
    StacksEdit,
    /// Remove a registration. Whatever is running keeps running.
    #[serde(rename = "stacks.forget")]
    StacksForget,
    /// Set and remove environment values. Values still never come back out.
    #[serde(rename = "env.write")]
    EnvWrite,
    /// Add and remove repositories.
    #[serde(rename = "repos.manage")]
    ReposManage,
    /// Add and remove credentials.
    #[serde(rename = "credentials.manage")]
    CredentialsManage,
    /// Remove unused images and containers.
    #[serde(rename = "cleanup.run")]
    CleanupRun,
    /// Open a shell in a container, which is as good as root on the host.
    #[serde(rename = "shell.open")]
    ShellOpen,
    /// Add, change and remove uptime checks.
    #[serde(rename = "checks.manage")]
    ChecksManage,
    /// Add, test and remove alert channels, and set resource alert rules.
    #[serde(rename = "alerts.manage")]
    AlertsManage,
}

impl Permission {
    /// The areas permissions are grouped under, looking before touching.
    pub const AREAS: [&'static str; 6] = [
        "Viewing",
        "Running stacks",
        "Changing stacks",
        "Sources",
        "Host",
        "Monitoring",
    ];

    pub const ALL: [Self; 20] = [
        Self::HostView,
        Self::ComposeRead,
        Self::LogsView,
        Self::ActivityView,
        Self::StacksDeploy,
        Self::StacksRestart,
        Self::StacksStop,
        Self::StacksTakeDown,
        Self::UpdatesCheck,
        Self::UpdatesAutoApply,
        Self::StacksCreate,
        Self::StacksEdit,
        Self::StacksForget,
        Self::EnvWrite,
        Self::ReposManage,
        Self::CredentialsManage,
        Self::CleanupRun,
        Self::ShellOpen,
        Self::ChecksManage,
        Self::AlertsManage,
    ];

    /// The stable name used on the wire and in storage.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HostView => "host.view",
            Self::ComposeRead => "stacks.read_compose",
            Self::LogsView => "logs.view",
            Self::ActivityView => "activity.view",
            Self::StacksDeploy => "stacks.deploy",
            Self::StacksRestart => "stacks.restart",
            Self::StacksStop => "stacks.stop",
            Self::StacksTakeDown => "stacks.take_down",
            Self::UpdatesCheck => "updates.check",
            Self::UpdatesAutoApply => "updates.auto_apply",
            Self::StacksCreate => "stacks.create",
            Self::StacksEdit => "stacks.edit",
            Self::StacksForget => "stacks.forget",
            Self::EnvWrite => "env.write",
            Self::ReposManage => "repos.manage",
            Self::CredentialsManage => "credentials.manage",
            Self::CleanupRun => "cleanup.run",
            Self::ShellOpen => "shell.open",
            Self::ChecksManage => "checks.manage",
            Self::AlertsManage => "alerts.manage",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == name)
    }

    /// The heading it is listed under when choosing.
    #[must_use]
    pub fn area(self) -> &'static str {
        match self {
            Self::HostView | Self::ComposeRead | Self::LogsView | Self::ActivityView => "Viewing",
            Self::StacksDeploy
            | Self::StacksRestart
            | Self::StacksStop
            | Self::StacksTakeDown
            | Self::UpdatesCheck
            | Self::UpdatesAutoApply => "Running stacks",
            Self::StacksCreate | Self::StacksEdit | Self::StacksForget | Self::EnvWrite => {
                "Changing stacks"
            }
            Self::ReposManage | Self::CredentialsManage => "Sources",
            Self::CleanupRun | Self::ShellOpen => "Host",
            Self::ChecksManage | Self::AlertsManage => "Monitoring",
        }
    }

    /// What granting it means, in a sentence a person can decide on.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::HostView => {
                "See stacks, containers, images, resource figures, history, update status and uptime checks"
            }
            Self::ComposeRead => "Read compose files, which may contain values pasted inline",
            Self::LogsView => "Read container logs, which often contain private details",
            Self::ActivityView => "Read the record of who did what",
            Self::StacksDeploy => "Deploy a stack, pulling images and applying changes",
            Self::StacksRestart => "Restart a stack's containers in place",
            Self::StacksStop => "Stop a stack's containers, leaving them in place",
            Self::StacksTakeDown => "Remove a stack's containers and networks; volumes are kept",
            Self::UpdatesCheck => "Check a stack for new commits and images now",
            Self::UpdatesAutoApply => "Turn automatic deploys of updates on or off",
            Self::StacksCreate => "Register new stacks",
            Self::StacksEdit => "Change a stack's compose file",
            Self::StacksForget => "Remove a stack from GhostDock, leaving its containers running",
            Self::EnvWrite => "Set and remove environment values (they still cannot be read back)",
            Self::ReposManage => "Add and remove repositories",
            Self::CredentialsManage => "Add and remove the credentials repositories use",
            Self::CleanupRun => "Remove unused images and containers",
            Self::ShellOpen => "Open a shell in any container, which is root on the host",
            Self::ChecksManage => "Add, change and remove uptime checks",
            Self::AlertsManage => {
                "Add, test and remove alert channels, and set resource alert rules"
            }
        }
    }
}

/// A token as listed. The secret itself is shown once, at creation, and
/// never again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: i64,
    pub name: String,
    /// The first characters of the secret, so a token found in a config
    /// file can be matched to its row here.
    pub prefix: String,
    pub permissions: Vec<Permission>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewApiToken {
    pub name: String,
    pub permissions: Vec<Permission>,
    /// None for a token that does not expire.
    pub expires_in_days: Option<u32>,
}

/// The response to creating a token: the only time the secret is sent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedApiToken {
    pub token: ApiToken,
    pub secret: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_permission_names_itself_the_same_way_in_storage_and_on_the_wire() {
        for p in Permission::ALL {
            assert_eq!(Permission::parse(p.as_str()), Some(p));
            assert_eq!(
                serde_json::to_value(p).unwrap(),
                serde_json::Value::String(p.as_str().to_owned())
            );
        }
        assert_eq!(Permission::parse("accounts.manage"), None);
    }

    #[test]
    fn every_permission_belongs_to_a_listed_area() {
        // The token form draws one group per listed area. A permission in an
        // unlisted area would have no checkbox and could never be granted.
        for p in Permission::ALL {
            assert!(Permission::AREAS.contains(&p.area()), "{p:?}");
        }
    }
}
