//! Stack registration and deployment wire types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Where a stack's compose file comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// YAML held by GhostDock itself.
    Inline,
    /// A compose file read from a Git repository at deploy time.
    Git,
}

/// A registered stack, as opposed to one merely observed on the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisteredStack {
    pub id: i64,
    pub host_id: i64,
    /// Compose project name; also the directory name on disk.
    pub slug: String,
    pub name: String,
    pub source_kind: SourceKind,
    /// Set for a Git-backed stack.
    pub git: Option<GitSource>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Where a Git-backed stack's compose file comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSource {
    pub repo_id: i64,
    pub repo_url: String,
    pub git_ref: String,
    pub compose_path: String,
    /// The commit last deployed, if any.
    pub last_commit: Option<String>,
}

/// What a deployment was trying to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Deploy,
    Stop,
    Restart,
    Remove,
}

impl Action {
    /// Present tense, for a button and for the toast that follows it.
    #[must_use]
    pub fn verb(self) -> &'static str {
        match self {
            Self::Deploy => "Deploy",
            Self::Stop => "Stop",
            Self::Restart => "Restart",
            Self::Remove => "Take down",
        }
    }
}

/// What caused a deployment to be attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Manual,
    Schedule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deployment {
    pub id: i64,
    pub stack_id: i64,
    pub action: Action,
    pub trigger: Trigger,
    pub status: DeploymentStatus,
    pub exit_code: Option<i32>,
    /// The commit this attempt deployed, for a Git-backed stack.
    pub commit_sha: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// A deployment together with the output compose produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentDetail {
    #[serde(flatten)]
    pub deployment: Deployment,
    pub log: String,
}

/// A stack's stored compose file.
///
/// Served separately from the stack itself: a listing does not need every
/// stack's YAML, and some of them are long.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackCompose {
    pub compose_yaml: String,
}

/// Request body for registering an inline stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewStack {
    pub name: String,
    pub compose_yaml: String,
}
