//! A shell session inside a container.
//!
//! Wraps the daemon's attached exec so that bollard stays inside this crate.
//! Callers see lines in and text out, which is all the server needs and
//! keeps the invariant that only this crate talks to Docker.

use std::pin::Pin;

use bollard::container::LogOutput;
use futures::{Stream, StreamExt};
use shared::logs::LogLine;
use tokio::io::{AsyncWrite, AsyncWriteExt};

type Output = Pin<Box<dyn Stream<Item = Result<LogOutput, bollard::errors::Error>> + Send>>;
type Input = Pin<Box<dyn AsyncWrite + Send>>;

/// An attached shell.
///
/// Dropping it closes the shell's stdin, which ends the session rather than
/// leaving a process attached to the daemon.
pub struct Shell {
    output: Output,
    input: Input,
}

impl std::fmt::Debug for Shell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shell").finish_non_exhaustive()
    }
}

impl Shell {
    pub(crate) fn new(output: Output, input: Input) -> Self {
        Self { output, input }
    }

    /// Splits into halves that can be driven independently.
    ///
    /// Both directions have to run at once -- output arrives while the user
    /// is typing -- and a single object owned by one task can only do one of
    /// them.
    #[must_use]
    pub fn split(self) -> (ShellReader, ShellWriter) {
        (
            ShellReader {
                output: self.output,
            },
            ShellWriter { input: self.input },
        )
    }
}

/// The output half.
pub struct ShellReader {
    output: Output,
}

impl ShellReader {
    /// The next batch of output, or `None` once the shell has ended.
    pub async fn next_lines(&mut self) -> Option<Vec<LogLine>> {
        let chunk = self.output.next().await?;
        Some(crate::logs::to_lines(&chunk.ok()?))
    }
}

/// The input half.
///
/// Dropping it closes the shell's stdin, which ends the session rather than
/// leaving a process attached to the daemon.
pub struct ShellWriter {
    input: Input,
}

impl ShellWriter {
    /// Sends input to the shell.
    pub async fn write(&mut self, text: &str) -> std::io::Result<()> {
        self.input.write_all(text.as_bytes()).await?;
        self.input.flush().await
    }
}
