//! Reading container output.

use bollard::container::LogOutput;
use shared::logs::{LogLine, Stream};

/// Longest line kept whole. Past it a line is cut, so a process that never
/// writes a newline cannot make its output accumulate without bound.
pub const MAX_LINE_BYTES: usize = 64 * 1024;

/// Splits one daemon chunk into lines, as if it were all there is.
///
/// A line cut off at the chunk's end is returned as it is. For a stream of
/// chunks, where a line can continue in the next, use [`Lines`].
#[must_use]
pub fn to_lines(chunk: &LogOutput) -> Vec<LogLine> {
    let mut lines = Lines::default();
    let mut out = lines.push(chunk);
    out.extend(lines.finish());
    out
}

/// Assembles lines from a stream of daemon chunks.
///
/// The daemon frames output arbitrarily, so a chunk is not a line: one
/// chunk may hold several, or half of one, or half a character. An
/// incomplete line is held until the rest of it arrives. Output and errors
/// are assembled apart, since their chunks interleave. Timestamps are
/// requested and then separated here, because a caller wants them as a
/// field rather than as a prefix it has to parse again.
#[derive(Debug, Default)]
pub struct Lines {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Lines {
    /// The lines `chunk` completes.
    pub fn push(&mut self, chunk: &LogOutput) -> Vec<LogLine> {
        let (stream, bytes) = match chunk {
            LogOutput::StdErr { message } => (Stream::Stderr, message),
            // Console and StdIn only appear for a TTY container, where the
            // daemon does not separate the streams at all. Treating them as
            // stdout matches what `docker logs` shows.
            LogOutput::StdOut { message }
            | LogOutput::Console { message }
            | LogOutput::StdIn { message } => (Stream::Stdout, message),
        };
        let pending = match stream {
            Stream::Stdout => &mut self.stdout,
            Stream::Stderr => &mut self.stderr,
        };

        let mut bytes: &[u8] = bytes;
        // With timestamps on, the daemon prefixes each piece of a line it
        // split, not only the first. The line keeps the first one.
        if !pending.is_empty()
            && timestamp_prefix(pending).is_some()
            && let Some(len) = timestamp_prefix(bytes)
        {
            bytes = &bytes[len..];
        }
        pending.extend_from_slice(bytes);

        let mut out = Vec::new();
        while let Some(end) = pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            emit(stream, &line[..end], &mut out);
        }
        while pending.len() > MAX_LINE_BYTES {
            let line: Vec<u8> = pending.drain(..MAX_LINE_BYTES).collect();
            emit(stream, &line, &mut out);
        }
        out
    }

    /// Whether part of a line is waiting for the rest.
    #[must_use]
    pub fn has_partial(&self) -> bool {
        !self.stdout.is_empty() || !self.stderr.is_empty()
    }

    /// Whatever is held, as lines: at the end of output, or when waiting
    /// longer for the rest would leave someone looking at nothing.
    pub fn finish(&mut self) -> Vec<LogLine> {
        let mut out = Vec::new();
        emit(Stream::Stdout, &std::mem::take(&mut self.stdout), &mut out);
        emit(Stream::Stderr, &std::mem::take(&mut self.stderr), &mut out);
        out
    }
}

fn emit(stream: Stream, bytes: &[u8], out: &mut Vec<LogLine>) {
    let text = String::from_utf8_lossy(bytes);
    let line = text.strip_suffix('\r').unwrap_or(&text);
    if !line.is_empty() {
        out.push(split_timestamp(stream, line));
    }
}

/// The length of a leading "timestamp " in `bytes`, if there is one.
fn timestamp_prefix(bytes: &[u8]) -> Option<usize> {
    let space = bytes.iter().take(40).position(|&b| b == b' ')?;
    let prefix = std::str::from_utf8(&bytes[..space]).ok()?;
    looks_like_timestamp(prefix).then_some(space + 1)
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
