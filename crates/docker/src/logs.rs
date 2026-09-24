//! Reading container output.

use bollard::container::LogOutput;
use shared::logs::{LogLine, Stream};

/// Splits a daemon log chunk into lines.
///
/// The daemon frames output arbitrarily, so a chunk is not a line: one
/// chunk may hold several, or half of one. Timestamps are requested and then
/// separated here, because a caller wants them as a field rather than as a
/// prefix it has to parse again.
#[must_use]
pub fn to_lines(chunk: &LogOutput) -> Vec<LogLine> {
    let (stream, bytes) = match chunk {
        LogOutput::StdErr { message } => (Stream::Stderr, message),
        // Console and StdIn only appear for a TTY container, where the
        // daemon does not separate the streams at all. Treating them as
        // stdout matches what `docker logs` shows.
        LogOutput::StdOut { message }
        | LogOutput::Console { message }
        | LogOutput::StdIn { message } => (Stream::Stdout, message),
    };

    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| split_timestamp(stream, line))
        .collect()
}

/// Separates an RFC 3339 timestamp prefix from the message.
fn split_timestamp(stream: Stream, line: &str) -> LogLine {
    match line.split_once(' ') {
        Some((prefix, rest)) if looks_like_timestamp(prefix) => LogLine {
            stream,
            at: Some(prefix.to_owned()),
            text: rest.to_owned(),
        },
        _ => LogLine {
            stream,
            at: None,
            text: line.to_owned(),
        },
    }
}

/// A deliberately shallow check.
///
/// The prefix is only split off when it plainly is one; a line of output
/// that merely starts with digits must not lose its first word.
fn looks_like_timestamp(prefix: &str) -> bool {
    prefix.len() >= 20
        && prefix.contains('T')
        && (prefix.ends_with('Z') || prefix.contains('+'))
        && prefix.starts_with(|c: char| c.is_ascii_digit())
}
