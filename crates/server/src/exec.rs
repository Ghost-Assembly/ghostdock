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

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::get;
use futures::{SinkExt, StreamExt};
use shared::logs::{LogLine, Stream};

use crate::auth::{Authorized, perm};
use crate::error::ApiError;
use crate::origin::SameOrigin;
use crate::state::AppState;

/// The shell to run.
///
/// `sh` rather than `bash`: it exists in Alpine, BusyBox and Debian alike,
/// and a session that fails because the image is small is a worse default
/// than one without line editing.
const SHELL: &str = "/bin/sh";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/hosts/{host_id}/containers/{id}/exec", get(open))
        .route(
            "/hosts/{host_id}/containers/{id}/exec/run",
            axum::routing::post(run),
        )
}

/// How often an open shell is pinged, as the event socket is.
const PING: std::time::Duration = std::time::Duration::from_secs(20);

const RUN_DEFAULT_SECS: u32 = 30;
const RUN_MAX_SECS: u32 = 300;
/// How much of a command the audit trail keeps.
const AUDITED_COMMAND_CHARS: usize = 200;

/// Runs one command and returns what it printed, for a program rather than
/// a person: nothing to attach, nothing left open.
///
/// As powerful as a shell, and gated the same way. The command itself goes
/// in the audit trail, as `sudo` logs commands: knowing a command was run
/// without knowing which is not much of a record.
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
    state
        .store
        .host_by_id(host_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let client = state.docker.clone().ok_or(ApiError::DaemonUnavailable)?;
    let wait = request
        .timeout_seconds
        .unwrap_or(RUN_DEFAULT_SECS)
        .clamp(1, RUN_MAX_SECS);

    crate::audit::record(
        &state,
        &principal,
        "run command",
        &id,
        Some(shared::short(command, AUDITED_COMMAND_CHARS)),
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
    state
        .store
        .host_by_id(host_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let client = state.docker.clone().ok_or(ApiError::DaemonUnavailable)?;

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
    let mut to_browser = tokio::spawn(async move {
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
