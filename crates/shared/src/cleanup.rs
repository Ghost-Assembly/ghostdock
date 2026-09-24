//! Reclaiming disk space.

use serde::{Deserialize, Serialize};

/// An image nothing is using.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnusedImage {
    pub id: String,
    /// Names the image is known by; empty for an untagged layer.
    pub tags: Vec<String>,
    pub size_bytes: u64,
    /// True when the image has no tags at all, usually a superseded build.
    pub dangling: bool,
}

impl UnusedImage {
    /// What to show a person: a name if it has one, else a short id.
    #[must_use]
    pub fn label(&self) -> String {
        self.tags.first().cloned().unwrap_or_else(|| {
            let short = self.id.strip_prefix("sha256:").unwrap_or(&self.id);
            format!("untagged {}", crate::short(short, 12))
        })
    }
}

/// A container that is not running and that nothing appears to need.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoppedContainer {
    pub id: String,
    pub name: String,
    pub image: String,
    /// The compose project it came from, if any.
    pub project: Option<String>,
    /// Docker's own summary, such as "Exited (0) 3 weeks ago".
    pub status: String,
}

/// What a cleanup would remove.
///
/// Always shown before anything is deleted. Reclaiming space is not urgent,
/// and deleting an image somebody still wanted is not undoable without a
/// download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CleanupPreview {
    /// Untagged images, nearly always safe to remove.
    pub dangling: Vec<UnusedImage>,
    /// Tagged images no container refers to.
    pub unused: Vec<UnusedImage>,
    /// Stopped containers from compose projects that are gone: not
    /// registered here, with nothing of theirs still running.
    #[serde(default)]
    pub leftover: Vec<StoppedContainer>,
    /// Stopped containers not created by Compose. More likely to be
    /// deliberate, so offered separately.
    #[serde(default)]
    pub standalone: Vec<StoppedContainer>,
}

impl CleanupPreview {
    #[must_use]
    pub fn dangling_bytes(&self) -> u64 {
        self.dangling.iter().map(|i| i.size_bytes).sum()
    }

    #[must_use]
    pub fn unused_bytes(&self) -> u64 {
        self.unused.iter().map(|i| i.size_bytes).sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dangling.is_empty()
            && self.unused.is_empty()
            && self.leftover.is_empty()
            && self.standalone.is_empty()
    }
}

/// What to remove. One group per request: each is its own decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupScope {
    /// Untagged images only.
    Dangling,
    /// Every image no container refers to.
    AllUnused,
    /// Stopped containers left by compose projects that are gone.
    Leftover,
    /// Stopped containers not created by Compose.
    Standalone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupRequest {
    pub scope: CleanupScope,
}

/// What a cleanup actually removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CleanupResult {
    pub removed: Vec<String>,
    pub reclaimed_bytes: u64,
    /// What could not be removed, usually because it was put back to use
    /// since the preview.
    pub kept: Vec<String>,
}
