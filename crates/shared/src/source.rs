//! Git remotes and the credentials that reach them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::deployment::RegisteredStack;

/// A stored credential, as exposed to clients.
///
/// Carries no secret. The secret is write-only across the API: it goes in
/// when created and is never returned, not even masked, because a masked
/// value still tells you its length.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    pub id: i64,
    pub name: String,
    pub username: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewCredential {
    pub name: String,
    /// Whatever the host expects beside a token: GitHub ignores it, GitLab
    /// wants `oauth2`.
    pub username: String,
    pub secret: String,
}

/// A Git remote. Several stacks commonly live in one repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    pub id: i64,
    pub url: String,
    pub credential_id: Option<i64>,
    /// Shown instead of the id, so the UI need not resolve it.
    pub credential_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewRepo {
    pub url: String,
    pub credential_id: Option<i64>,
}

/// Request body for registering a Git-backed stack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewGitStack {
    pub name: String,
    pub repo_id: i64,
    /// A full ref, e.g. `refs/heads/main`. Stored as given so an ambiguous
    /// short name cannot resolve to a tag on one poll and a branch on the next.
    pub git_ref: String,
    /// Path to the compose file within the repository.
    pub compose_path: String,
}

/// Environment variables belonging to a stack.
///
/// Listing returns keys alone: values are secrets, and the point of sealing
/// them is undone by an endpoint that hands them back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackEnvKeys {
    pub keys: Vec<String>,
}

/// Replaces a stack's environment wholesale.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackEnv {
    pub vars: Vec<EnvVar>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

/// Body for setting a single variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvValue {
    pub value: String,
}

/// Changing which credential a repository is reached with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoCredential {
    /// None for a public repository.
    pub credential_id: Option<i64>,
}

/// Looking for compose files in a repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverRequest {
    /// A full ref, e.g. `refs/heads/main`.
    pub git_ref: String,
    /// Paths to look for, as a glob. Omitted for the usual layouts.
    pub pattern: Option<String>,
}

/// What discovery found at one commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    pub commit: String,
    /// The pattern used, so a default can be shown and then edited.
    pub pattern: String,
    pub found: Vec<Discovered>,
}

/// One compose file, and what registering it would mean.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovered {
    pub path: String,
    /// The name it would be registered under.
    pub name: String,
    pub status: DiscoveredStatus,
    /// The bundled icon its name suggests, as for a stack; the file itself
    /// is not read.
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiscoveredStatus {
    /// Not registered yet, and the name is free.
    New,
    /// This file is already a registered stack.
    Registered { stack_id: i64, stack_name: String },
    /// Another stack already uses the name this one would get.
    NameTaken { stack_name: String },
}

/// Registering some of what discovery found.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRequest {
    pub git_ref: String,
    pub pattern: Option<String>,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportResult {
    pub created: Vec<RegisteredStack>,
    /// Paths not registered, each with the reason.
    pub skipped: Vec<(String, String)>,
}
