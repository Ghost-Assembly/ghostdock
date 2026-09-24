//! Container output.

use serde::{Deserialize, Serialize};

/// Which stream a line came from.
///
/// Kept apart rather than merged into one blob: when something has gone
/// wrong the answer is almost always on stderr, and a reader needs to find
/// it without reading everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    pub stream: Stream,
    /// Timestamp as the daemon reported it, when it did.
    pub at: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Logs {
    pub container: String,
    pub lines: Vec<LogLine>,
    /// True when older output exists beyond what was returned.
    pub truncated: bool,
}

/// Picking up a followed log where a snapshot of it ended.
///
/// The daemon's `since` is whole seconds, so following from the newest line
/// shown replays that whole second, and the repeats must be dropped. They
/// cannot be told apart by timestamp alone: the daemon copies stdout and
/// stderr separately and stamps each line as it reads it, so a log's order
/// is not always its timestamps' order, and "anything no newer than the last
/// line" both repeats lines and drops them. What the snapshot already holds
/// from that second is remembered exactly instead, and only those are
/// dropped.
#[derive(Debug, Clone, Default)]
pub struct Resume {
    since: Option<i64>,
    seen: std::collections::HashSet<(String, Stream, String)>,
}

impl Resume {
    #[must_use]
    pub fn after<'a, I>(snapshot: I) -> Self
    where
        I: IntoIterator<Item = &'a LogLine>,
        I::IntoIter: Clone,
    {
        let snapshot = snapshot.into_iter();
        let newest = snapshot
            .clone()
            .filter_map(|line| stamp(line).map(|t| t.timestamp()))
            .max();
        let Some(since) = newest else {
            return Self::default();
        };
        let seen = snapshot
            .filter(|line| stamp(line).is_some_and(|t| t.timestamp() >= since))
            .filter_map(|line| {
                line.at
                    .clone()
                    .map(|at| (at, line.stream, line.text.clone()))
            })
            .collect();
        Self {
            since: Some(since),
            seen,
        }
    }

    /// Whole seconds to follow from, or none for "from now on".
    #[must_use]
    pub fn since(&self) -> Option<i64> {
        self.since
    }

    /// Whether a line from the stream is one the snapshot did not show.
    #[must_use]
    pub fn is_new(&self, line: &LogLine) -> bool {
        match &line.at {
            Some(at) => !self
                .seen
                .contains(&(at.clone(), line.stream, line.text.clone())),
            None => true,
        }
    }
}

fn stamp(line: &LogLine) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    line.at
        .as_deref()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(stream: Stream, at: &str, text: &str) -> LogLine {
        LogLine {
            stream,
            at: Some(at.to_owned()),
            text: text.to_owned(),
        }
    }

    #[test]
    fn follows_from_the_second_of_the_newest_line_not_the_last() {
        // Written in this order, stamped the other way round.
        let snapshot = [
            line(Stream::Stdout, "2026-09-24T01:00:12.600Z", "tick 4"),
            line(Stream::Stderr, "2026-09-24T01:00:12.500Z", "warn 4"),
        ];
        let resume = Resume::after(&snapshot);
        let twelve = chrono::DateTime::parse_from_rfc3339("2026-09-24T01:00:12Z")
            .map(|t| t.timestamp())
            .ok();
        assert_eq!(resume.since(), twelve);
    }

    #[test]
    fn a_replayed_line_is_dropped_whatever_order_it_comes_in() {
        let snapshot = [
            line(Stream::Stdout, "2026-09-24T01:00:12.600Z", "tick 4"),
            line(Stream::Stderr, "2026-09-24T01:00:12.500Z", "warn 4"),
        ];
        let resume = Resume::after(&snapshot);
        // The stream replays the second in timestamp order.
        assert!(!resume.is_new(&snapshot[1]));
        assert!(!resume.is_new(&snapshot[0]));
    }

    #[test]
    fn a_line_from_the_same_second_that_the_snapshot_missed_is_kept() {
        // Stamped before the newest line shown but written after the
        // snapshot was taken. A timestamp cut-off would lose it.
        let snapshot = [line(Stream::Stdout, "2026-09-24T01:00:12.600Z", "tick 4")];
        let resume = Resume::after(&snapshot);
        assert!(resume.is_new(&line(Stream::Stderr, "2026-09-24T01:00:12.550Z", "warn 4")));
    }

    #[test]
    fn identical_text_at_another_moment_is_a_new_line() {
        let snapshot = [line(Stream::Stdout, "2026-09-24T01:00:12.600Z", "ok")];
        let resume = Resume::after(&snapshot);
        assert!(resume.is_new(&line(Stream::Stdout, "2026-09-24T01:00:12.700Z", "ok")));
    }

    #[test]
    fn with_nothing_shown_everything_is_new_and_following_starts_now() {
        let resume = Resume::after(&[] as &[LogLine]);
        assert_eq!(resume.since(), None);
        assert!(resume.is_new(&line(Stream::Stdout, "2026-09-24T01:00:12.600Z", "x")));
    }
}

/// Running one command in a container, without an interactive shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCommand {
    /// Run with `sh -c`, so pipes and redirections work.
    pub command: String,
    /// How long to wait for it. The default is 30 seconds, the most 300.
    pub timeout_seconds: Option<u32>,
}

/// What a command printed and how it ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandResult {
    /// None if it had not finished when the wait ran out.
    pub exit_code: Option<i64>,
    pub output: Vec<LogLine>,
    /// True when output beyond the limit was dropped.
    pub truncated: bool,
    /// True when the wait ran out. The command may still be running.
    pub timed_out: bool,
}
