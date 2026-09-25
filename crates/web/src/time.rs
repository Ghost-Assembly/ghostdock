//! Times as the person reading them keeps time.
//!
//! The server speaks UTC. Shown as it arrives, a deploy at nine in the
//! morning read as seven, beside a chart whose axis said nine. Every time on
//! screen goes through here, so they all agree with each other and with the
//! clock on the wall.

use chrono::{DateTime, Local, Utc};

/// `at` in this device's time zone, as `format` lays it out.
#[must_use]
pub fn local(at: DateTime<Utc>, format: &str) -> String {
    at.with_timezone(&Local).format(format).to_string()
}

/// A Unix timestamp in this device's time zone; empty if out of range.
#[must_use]
pub fn local_timestamp(t: i64, format: &str) -> String {
    DateTime::from_timestamp(t, 0).map_or_else(String::new, |at| local(at, format))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_is_shown_in_the_local_zone() {
        let at = DateTime::from_timestamp(1_700_000_000, 0).unwrap_or_default();
        let expected = at.with_timezone(&Local).format("%d %b %H:%M").to_string();
        assert_eq!(local(at, "%d %b %H:%M"), expected);
        assert_eq!(local_timestamp(1_700_000_000, "%d %b %H:%M"), expected);
        assert_eq!(local_timestamp(i64::MAX, "%H:%M"), "");
    }
}
