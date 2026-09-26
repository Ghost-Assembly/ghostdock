//! A shell inside a container.
//!
//! Line-oriented rather than a terminal emulator. The exec runs without a
//! TTY, so the daemon returns clean output with no escape sequences to
//! interpret, and the client is a scrollback plus an input box. On a phone
//! that is the better shape: a soft keyboard makes cursor-addressed
//! programs unusable anyway, and what people do here is run a command and
//! read what it says.
//!
//! The cost, stated plainly: `vi` and `top` will not work.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use shared::logs::{LogLine, Stream};
use shared::reference::Access;
use shared::token::Permission;

use crate::auth::{Authorized, perm};
use crate::error::ApiError;
use crate::origin::SameOrigin;
use crate::reference::Routes;
use crate::state::AppState;

/// The shell to run.
///
/// `sh` rather than `bash`: it exists in Alpine, BusyBox and Debian alike,
/// and a session that fails because the image is small is a worse default
/// than one without line editing.
const SHELL: &str = "/bin/sh";

pub fn routes() -> Routes {
    let shell = Access::Token(Permission::ShellOpen);
    Routes::new("Shell")
        .get(
            "/hosts/{host_id}/containers/{id}/exec",
            shell,
            "A line-oriented shell in a container over a WebSocket",
            open,
        )
        .post(
            "/hosts/{host_id}/containers/{id}/exec/run",
            shell,
            "Runs one command in a container and returns its output; the program run is audited",
            run,
        )
}

const RUN_DEFAULT_SECS: u32 = 30;
const RUN_MAX_SECS: u32 = 300;
/// How much of a program's name the audit trail keeps.
const AUDITED_PROGRAM_CHARS: usize = 64;

/// Runs one command and returns what it printed, for a program rather than
/// a person: nothing to attach, nothing left open.
///
/// As powerful as a shell, and gated the same way. The audit trail records
/// which program ran and how many arguments it had, but never the text:
/// see [`audited`].
async fn run(
    principal: Authorized<perm::ShellOpen>,
    State(state): State<AppState>,
    Path((host_id, id)): Path<(i64, String)>,
    axum::Json(request): axum::Json<shared::logs::RunCommand>,
) -> Result<axum::Json<shared::logs::CommandResult>, ApiError> {
    let command = request.command.trim();
    if command.is_empty() {
        return Err(ApiError::BadRequest("Give a command to run.".to_owned()));
    }
    let client = crate::hosts::daemon(&state, host_id).await?;
    let wait = request
        .timeout_seconds
        .unwrap_or(RUN_DEFAULT_SECS)
        .clamp(1, RUN_MAX_SECS);

    crate::audit::record(
        &state,
        &principal,
        "run command",
        &id,
        Some(&audited(command)),
    )
    .await;
    let result = client
        .run_command(
            &id,
            command,
            std::time::Duration::from_secs(u64::from(wait)),
        )
        .await?;
    Ok(axum::Json(result))
}

/// What the audit trail says of a command: the program and how many
/// arguments follow it, never the text. Arguments are where passwords and
/// tokens go (`mysql -p…`, `curl -H 'Authorization: …'`), and a trail that
/// quoted them would be the easiest place in the product to read one.
///
/// Words are split on whitespace without the shell's quoting, so a quoted
/// argument counts once per word in it. Leading `NAME=value` words set the
/// environment rather than name the program, and often carry a secret, so
/// they are skipped and not counted.
fn audited(command: &str) -> String {
    let mut words = command.split_whitespace().skip_while(|w| is_assignment(w));
    let Some(program) = words.next() else {
        return "variable assignments only".to_owned();
    };
    let program = shared::short(program, AUDITED_PROGRAM_CHARS);
    match words.count() {
        0 => format!("{program}, no arguments"),
        1 => format!("{program}, 1 argument"),
        n => format!("{program}, {n} arguments"),
    }
}

/// Whether a word is a shell variable assignment, `NAME=value`.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Upgrades to a WebSocket carrying one shell session.
///
/// Authentication happens here, before the upgrade: the browser sends its
/// session cookie with the handshake, and a socket that reaches a container
/// shell must never be reachable without one.
async fn open(
    principal: Authorized<perm::ShellOpen>,
    // Before the upgrade extractor, so a cross-origin handshake is refused
    // during extraction rather than relying on the handler body running.
    // WebSocket handshakes are not covered by the same-origin policy the way
    // fetch and EventSource are: any page can open a socket here and the
    // browser will attach the session cookie.
    _origin: SameOrigin,
    State(state): State<AppState>,
    Path((host_id, id)): Path<(i64, String)>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let client = crate::hosts::daemon(&state, host_id).await?.clone();

    // The most powerful thing anyone can do here, so it is recorded before
    // the socket is handed over rather than after it closes.
    crate::audit::record(&state, &principal, "open shell", &id, None).await;

    let revoked = state.revocations.until_revoked(&principal);
    Ok(upgrade.on_upgrade(move |socket| session(socket, client, id, revoked)))
}

async fn session(
    socket: WebSocket,
    client: docker::Client,
    container: String,
    revoked: impl std::future::Future<Output = ()> + Send + 'static,
) {
    let shell = match client.start_shell(&container, SHELL).await {
        Ok(shell) => shell,
        Err(e) => {
            let _ = notify(socket, &format!("could not start a shell: {e}")).await;
            return;
        }
    };

    let (mut sink, mut incoming) = socket.split();
    let (mut reader, mut writer) = shell.split();

    // Told why, when the session is ended from this side.
    let (stop, mut stopped) = tokio::sync::oneshot::channel::<&'static str>();

    // Container output to the browser, and a ping when there is none: a
    // proxy drops a connection that stays quiet, and a shell often does.
    // Not `socket::pump`: input goes to the shell while output is sent, and
    // a revoked shell says why before it closes.
    let mut to_browser = tokio::spawn(async move {
        use crate::socket::PING;
        let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING, PING);
        loop {
            tokio::select! {
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                        return;
                    }
                }
                reason = &mut stopped => {
                    if let Ok(reason) = reason {
                        let _ = send_line(&mut sink, reason).await;
                        let _ = sink.close().await;
                    }
                    return;
                }
                next = reader.next_lines() => {
                    let Some(lines) = next else { break };
                    for line in lines {
                        let Ok(json) = serde_json::to_string(&line) else {
                            continue;
                        };
                        if sink.send(Message::Text(json.into())).await.is_err() {
                            return;
                        }
                    }
                }
            }
        }
        // The shell ended; say so rather than leaving a dead box on screen.
        let _ = send_line(&mut sink, "session ended").await;
    });

    // Browser input to the shell. A newline is appended if the client did
    // not send one, since a line-oriented client sends commands, not
    // keystrokes, and a shell waits for the newline before acting.
    //
    // A revoked credential ends the shell mid-session: this is the one
    // connection that must not outlive the right to it.
    tokio::pin!(revoked);
    loop {
        let message = tokio::select! {
            () = &mut revoked => {
                tracing::info!(container, "closed a shell whose access was revoked");
                let _ = stop.send("access was revoked; this shell is closed");
                drop(writer);
                // Long enough for the explanation to be sent, no longer. A
                // browser that will not take it is cut off, not left
                // holding the shell's output stream open.
                if tokio::time::timeout(std::time::Duration::from_secs(2), &mut to_browser)
                    .await
                    .is_err()
                {
                    to_browser.abort();
                }
                return;
            }
            next = incoming.next() => match next {
                Some(Ok(message)) => message,
                _ => break,
            },
        };
        let Message::Text(text) = message else {
            if matches!(message, Message::Close(_)) {
                break;
            }
            continue;
        };

        let line = if text.ends_with('\n') {
            text.to_string()
        } else {
            format!("{text}\n")
        };
        if writer.write(&line).await.is_err() {
            break;
        }
    }

    // Closing stdin ends the shell rather than leaving it attached.
    drop(writer);
    to_browser.abort();
}

async fn send_line<S>(sink: &mut S, text: &str) -> Result<(), axum::Error>
where
    S: SinkExt<Message, Error = axum::Error> + Unpin,
{
    let line = LogLine {
        stream: Stream::Stderr,
        at: None,
        text: text.to_owned(),
    };
    match serde_json::to_string(&line) {
        Ok(json) => sink.send(Message::Text(json.into())).await,
        Err(_) => Ok(()),
    }
}

/// Sends one explanatory line and closes.
async fn notify(mut socket: WebSocket, message: &str) -> Result<(), axum::Error> {
    let line = LogLine {
        stream: Stream::Stderr,
        at: None,
        text: message.to_owned(),
    };
    if let Ok(json) = serde_json::to_string(&line) {
        socket.send(Message::Text(json.into())).await?;
    }
    socket.close().await
}

#[cfg(test)]
mod tests {
    use super::audited;

    #[test]
    fn a_command_is_recorded_as_its_program_and_argument_count() {
        assert_eq!(audited("id"), "id, no arguments");
        assert_eq!(audited("ls /data"), "ls, 1 argument");
        assert_eq!(
            audited("mysql -u root -phunter2 shop"),
            "mysql, 4 arguments"
        );
        assert_eq!(
            audited("TOKEN=hunter2 A_1=x curl -s http://db"),
            "curl, 2 arguments"
        );
        assert_eq!(audited("TOKEN=hunter2"), "variable assignments only");
        // Not an assignment: `=` in a word that cannot be a name.
        assert_eq!(audited("--opt=x run"), "--opt=x, 1 argument");
    }
}
