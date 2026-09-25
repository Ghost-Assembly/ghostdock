//! Turning daemon log chunks into lines.

use bollard::container::LogOutput;
use bytes::Bytes;
use docker::logs::{Lines, MAX_LINE_BYTES, to_lines};
use shared::logs::Stream;

fn stdout(text: &str) -> LogOutput {
    LogOutput::StdOut {
        message: Bytes::from(text.to_owned()),
    }
}

#[test]
fn one_chunk_can_hold_several_lines() {
    // The daemon frames output arbitrarily; a chunk is not a line.
    let lines = to_lines(&stdout("first\nsecond\nthird\n"));
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[1].text, "second");
}

#[test]
fn stderr_stays_distinguishable() {
    // When something has gone wrong the answer is usually here, and a
    // reader needs to find it without reading everything.
    let lines = to_lines(&LogOutput::StdErr {
        message: Bytes::from_static(b"panic: something broke\n"),
    });
    assert_eq!(lines[0].stream, Stream::Stderr);
    assert_eq!(lines[0].text, "panic: something broke");
}

#[test]
fn a_timestamp_prefix_becomes_a_field() {
    let lines = to_lines(&stdout("2026-09-23T10:11:12.345678901Z starting up\n"));
    assert_eq!(
        lines[0].at.as_deref(),
        Some("2026-09-23T10:11:12.345678901Z")
    );
    assert_eq!(lines[0].text, "starting up");
}

#[test]
fn a_line_that_merely_starts_with_digits_keeps_its_first_word() {
    // The check is deliberately shallow, so ordinary output is not mangled.
    for text in [
        "200 GET /health",
        "12345 requests served",
        "2026 was a year",
    ] {
        let lines = to_lines(&stdout(&format!("{text}\n")));
        assert_eq!(lines[0].at, None, "{text:?} has no timestamp");
        assert_eq!(lines[0].text, text);
    }
}

#[test]
fn a_tty_container_reports_on_stdout() {
    // With a TTY the daemon does not separate the streams, and `docker logs`
    // shows it all as output.
    let lines = to_lines(&LogOutput::Console {
        message: Bytes::from_static(b"interactive\n"),
    });
    assert_eq!(lines[0].stream, Stream::Stdout);
}

#[test]
fn blank_lines_are_dropped_rather_than_padding_the_view() {
    let lines = to_lines(&stdout("one\n\n\ntwo\n"));
    assert_eq!(lines.len(), 2);
}

fn stderr(text: &str) -> LogOutput {
    LogOutput::StdErr {
        message: Bytes::from(text.to_owned()),
    }
}

fn texts(lines: &[shared::logs::LogLine]) -> Vec<&str> {
    lines.iter().map(|l| l.text.as_str()).collect()
}

#[test]
fn a_line_split_across_chunks_is_one_line() {
    // A process writes when it likes; the daemon forwards each write as it
    // comes, so half a line in one chunk is ordinary.
    let mut lines = Lines::default();
    assert!(lines.push(&stdout("downloading lay")).is_empty());
    assert_eq!(
        texts(&lines.push(&stdout("er 3\nnext\npart"))),
        ["downloading layer 3", "next"]
    );
    assert_eq!(
        texts(&lines.finish()),
        ["part"],
        "the end of output ends the line"
    );
    assert!(lines.finish().is_empty());
}

#[test]
fn a_character_split_across_chunks_survives() {
    let mut lines = Lines::default();
    let bytes = "café\n".as_bytes();
    let (head, tail) = bytes.split_at(4); // inside the é
    assert!(
        lines
            .push(&LogOutput::StdOut {
                message: Bytes::copy_from_slice(head)
            })
            .is_empty()
    );
    let out = lines.push(&LogOutput::StdOut {
        message: Bytes::copy_from_slice(tail),
    });
    assert_eq!(texts(&out), ["café"]);
}

#[test]
fn output_and_errors_are_assembled_apart() {
    let mut lines = Lines::default();
    assert!(lines.push(&stdout("out ")).is_empty());
    let err = lines.push(&stderr("err\n"));
    assert_eq!(err[0].stream, Stream::Stderr);
    assert_eq!(texts(&err), ["err"]);
    let out = lines.push(&stdout("done\n"));
    assert_eq!(out[0].stream, Stream::Stdout);
    assert_eq!(texts(&out), ["out done"]);
}

#[test]
fn a_continued_line_keeps_its_first_timestamp_only() {
    // With timestamps on, the daemon prefixes every piece of a long line it
    // split, not only the first.
    let mut lines = Lines::default();
    assert!(
        lines
            .push(&stdout("2026-09-23T10:11:12.000000001Z first half"))
            .is_empty()
    );
    let out = lines.push(&stdout("2026-09-23T10:11:12.000000002Z , second half\n"));
    assert_eq!(out[0].at.as_deref(), Some("2026-09-23T10:11:12.000000001Z"));
    assert_eq!(out[0].text, "first half, second half");
}

#[test]
fn a_line_without_end_is_cut_rather_than_held_forever() {
    let mut lines = Lines::default();
    let chunk = "x".repeat(MAX_LINE_BYTES / 4);
    let mut out = Vec::new();
    for _ in 0..8 {
        out.extend(lines.push(&stdout(&chunk)));
    }
    assert!(!out.is_empty(), "nothing was let through");
    assert!(out.iter().all(|l| l.text.len() <= MAX_LINE_BYTES));
}

#[test]
fn invalid_utf8_does_not_lose_the_line() {
    // A container emitting raw bytes should still be readable, not silent.
    let lines = to_lines(&LogOutput::StdOut {
        message: Bytes::from_static(b"before \xff\xfe after\n"),
    });
    assert_eq!(lines.len(), 1);
    assert!(lines[0].text.starts_with("before "));
    assert!(lines[0].text.ends_with(" after"));
}
