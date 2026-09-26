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
/// Words are split on space, tab and newline without the shell's quoting,
/// so a quoted argument counts once per word in it. Leading `NAME=value`
/// words set the environment rather than name the program, and often carry
/// a secret, so they are skipped and not counted.
///
/// The shell is not being parsed, so anything but plain characters before
/// the program (a quote, a backslash, `$(…)`, a backtick) could carry part
/// of a value into what looks like the program: `A="x y" cmd` reads `y"`,
/// and `A=$(cat /run/secrets/x ) cmd` reads the secret's path. So the
/// program is named only when every word up to and including it is plain;
/// otherwise the entry says only how many words the command had. After the
/// program nothing is read but the count.
fn audited(command: &str) -> String {
    // Split where sh's default IFS splits, space, tab and newline, and
    // nowhere else: other whitespace (a no-break space, a bare `\r`) stays
    // inside its word, as it does in the shell, so it cannot cut a value in
    // two and leave its tail looking like the program.
    let words: Vec<&str> = command
        .split([' ', '\t', '\n'])
        .filter(|w| !w.is_empty())
        .collect();
    let skipped = words.iter().take_while(|w| is_assignment(w)).count();
    let Some(program) = words.get(skipped) else {
        return "variable assignments only".to_owned();
    };
    if !words.get(..=skipped).is_some_and(plain_up_to_program) {
        return match words.len() {
            1 => "a command, 1 word".to_owned(),
            n => format!("a command, {n} words"),
        };
    }
    let program = shared::short(program, AUDITED_PROGRAM_CHARS);
    match words.len() - skipped - 1 {
        0 => format!("{program}, no arguments"),
        1 => format!("{program}, 1 argument"),
        n => format!("{program}, {n} arguments"),
    }
}

/// Whether the leading assignments and the program that ends `leading` are
/// all plain: the program entirely, and each assignment a name, one `=`,
/// and a plain (possibly empty) value. An allowlist, so anything the shell
/// might treat specially disqualifies without having to be named.
fn plain_up_to_program(leading: &[&str]) -> bool {
    let Some((program, assignments)) = leading.split_last() else {
        return false;
    };
    is_plain(program)
        && assignments.iter().all(|word| {
            word.split_once('=')
                .is_some_and(|(name, value)| is_name(name) && is_plain(value))
        })
}

/// Whether every character is in `[A-Za-z0-9_./+-]`.
fn is_plain(word: &str) -> bool {
    word.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '+' | '-'))
}

/// Whether a word has the shape of a shell variable assignment, `NAME=…`,
/// whatever follows the `=`.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| is_name(name))
}

/// Whether `name` is a shell variable name.
fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
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
        assert_eq!(audited("psql -h db"), "psql, 2 arguments");
    }

    #[test]
    fn quoting_that_could_hide_a_secret_leaves_only_a_word_count() {
        // Words are split without the shell's quoting, so a quoted or
        // escaped assignment can spill a fragment of its value into what
        // looks like the program.
        assert_eq!(
            audited(r#"PGPASSWORD="correct horse" psql -h db"#),
            "a command, 5 words"
        );
        assert_eq!(audited("'TOKEN=s3cret' ./run"), "a command, 2 words");
        assert_eq!(audited(r"TOKEN=a\ b cmd"), "a command, 3 words");
        // Not a plain program name.
        assert_eq!(audited("--opt=x run"), "a command, 2 words");
        assert_eq!(audited("\"x\""), "a command, 1 word");
    }

    #[test]
    fn a_substitution_before_the_program_leaves_only_a_word_count() {
        // Its output is the value, and its words can land where the
        // program would be read.
        assert_eq!(
            audited("TOKEN=$(cat /run/secrets/db_password ) cmd args"),
            "a command, 5 words"
        );
        assert_eq!(
            audited("TOKEN=`cat /run/secrets/db_password ` cmd args"),
            "a command, 5 words"
        );
        assert_eq!(audited("A=1 B=$(id -un ) cmd"), "a command, 5 words");
        assert_eq!(
            audited("PASSWORD=$(cat /run/secrets/x) psql"),
            "a command, 3 words"
        );
        assert_eq!(audited("TOKEN=`cat` psql"), "a command, 2 words");
        assert_eq!(audited("X=a=b cmd"), "a command, 2 words");
        assert_eq!(audited("~/bin/run"), "a command, 1 word");
        assert_eq!(audited("TOKEN=\u{e9} cmd"), "a command, 2 words");
    }

    #[test]
    fn words_split_only_where_the_shell_splits_them() {
        // sh splits on space, tab and newline. Any other whitespace is part
        // of the word, so a value holding one must not be cut in two.
        let nbsp = audited("PGPASSWORD=hunter\u{a0}2-db.prod psql -h db");
        assert_eq!(nbsp, "a command, 4 words");
        assert!(!nbsp.contains("hunter") && !nbsp.contains("db.prod"));
        let cr = audited("TOKEN=x\ry cmd");
        assert_eq!(cr, "a command, 2 words");
        assert!(!cr.contains('y'));
        assert_eq!(audited("TOKEN=a\u{3000}b cmd"), "a command, 2 words");
        assert_eq!(audited("A=1\tpsql\t-h\ndb"), "psql, 2 arguments");
    }

    #[test]
    fn a_substitution_after_the_program_is_only_counted() {
        assert_eq!(audited("cmd $(echo x)"), "cmd, 2 arguments");
        assert_eq!(audited("EMPTY= cmd `id`"), "cmd, 1 argument");
    }
}
