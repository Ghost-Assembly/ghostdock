//! What is waiting to be applied to a stack.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::deployment::RegisteredStack;

/// One image a stack runs, and whether its tag has moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageStatus {
    /// The reference as the compose file wrote it.
    pub image: String,
    /// Digest of what is running, when it could be determined.
    pub running: Option<String>,
    /// Digest the tag points at now.
    pub available: Option<String>,
    /// Why this image could not be checked, if it could not be.
    pub error: Option<String>,
}

impl ImageStatus {
    /// True only when both digests are known and differ.
    ///
    /// An unknown digest is never an update: reporting one because a check
    /// failed would send people chasing changes that do not exist, and they
    /// would stop believing the list.
    #[must_use]
    pub fn has_moved(&self) -> bool {
        match (&self.running, &self.available) {
            (Some(running), Some(available)) => running != available,
            _ => false,
        }
    }
}

/// Everything known about pending changes to one stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UpdateStatus {
    pub checked_at: Option<DateTime<Utc>>,
    /// The commit the tracked ref points at now, for a Git-backed stack.
    pub remote_commit: Option<String>,
    /// The commit last deployed.
    pub deployed_commit: Option<String>,
    pub images: Vec<ImageStatus>,
    /// Why the last check failed, if it did.
    pub error: Option<String>,
}

impl UpdateStatus {
    /// True when the repository has moved ahead of what was deployed.
    #[must_use]
    pub fn is_behind_git(&self) -> bool {
        match (&self.remote_commit, &self.deployed_commit) {
            (Some(remote), Some(deployed)) => remote != deployed,
            // A stack with a remote commit and nothing deployed has never
            // been deployed; that is a state to show, not an update to apply.
            _ => false,
        }
    }

    #[must_use]
    pub fn moved_images(&self) -> Vec<&ImageStatus> {
        self.images.iter().filter(|i| i.has_moved()).collect()
    }

    /// True when there is anything to apply.
    #[must_use]
    pub fn has_update(&self) -> bool {
        self.is_behind_git() || !self.moved_images().is_empty()
    }
}

/// One row of the Updates view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackUpdate {
    pub stack: RegisteredStack,
    pub status: UpdateStatus,
    /// Plain-language summary, computed server-side.
    ///
    /// Computed there because the wording is a rule about what an update
    /// means, and a second implementation in the client would drift from it.
    pub reason: Option<String>,
    /// Whether the stack applies updates without being asked.
    pub auto_apply: bool,
    /// Whether an operation is running for it right now.
    pub busy: bool,
}

/// Body for turning automatic application on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoApply {
    pub enabled: bool,
}
