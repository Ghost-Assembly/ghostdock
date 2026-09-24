//! Healthcheck fallback parsing.
//!
//! Modern daemons report health as a structured field on the container list.
//! Older ones only embed it in the human-readable status, e.g.
//! `Up 2 hours (healthy)`. This is the fallback for those, so health does not
//! silently vanish from the UI on an older Docker.

use shared::container::Health;

/// Extracts health from a daemon status string, if it carries one.
#[must_use]
pub fn from_status(status: &str) -> Option<Health> {
    let lowered = status.to_ascii_lowercase();
    // Order matters: "unhealthy" contains "healthy", so the more specific
    // pattern has to be tested first or a failing container reads as green.
    if lowered.contains("(unhealthy") {
        Some(Health::Unhealthy)
    } else if lowered.contains("(health: starting") {
        Some(Health::Starting)
    } else if lowered.contains("(healthy") {
        Some(Health::Healthy)
    } else {
        None
    }
}
