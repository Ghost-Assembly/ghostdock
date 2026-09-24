//! Reading logs and reclaiming space.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use shared::cleanup::{CleanupPreview, CleanupRequest, CleanupResult, CleanupScope};
use shared::logs::Logs;

use crate::auth::{Authorized, perm};
use crate::error::ApiError;
use crate::state::AppState;

/// Lines returned when the caller does not say.
const DEFAULT_TAIL: usize = 500;

/// Most that will be returned however many are asked for.
///
/// A long-running container's log can be hundreds of megabytes, and sending
/// it to a phone to find the last twenty lines helps nobody.
const MAX_TAIL: usize = 5_000;

#[derive(Debug, Deserialize)]
pub struct LogQuery {
    tail: Option<usize>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/hosts/{host_id}/containers/{id}/logs", get(logs))
        .route("/hosts/{host_id}/containers/{id}/logs.txt", get(logs_text))
        .route("/hosts/{host_id}/containers/{id}/logs/follow", get(follow))
        .route(
            "/hosts/{host_id}/containers/{id}/logs/socket",
            get(follow_socket),
        )
        .route("/hosts/{host_id}/cleanup", get(preview).post(run_cleanup))
}

async fn logs(
    _principal: Authorized<perm::LogsView>,
    State(state): State<AppState>,
    Path((host_id, id)): Path<(i64, String)>,
    Query(query): Query<LogQuery>,
) -> Result<Json<Logs>, ApiError> {
    let client = client_for(&state, host_id).await?;
    let tail = query.tail.unwrap_or(DEFAULT_TAIL).clamp(1, MAX_TAIL);
    Ok(Json(client.container_logs(&id, tail).await?))
}

#[derive(Debug, Deserialize)]
pub struct FollowQuery {
    /// Continue from this Unix time, typically the last line already shown.
    since: Option<i64>,
}

/// New output as it is written, as server-sent events: `line` for each line,
/// then `end` once the container stops.
///
/// A second stream besides `/events`, open only while someone is watching
/// one container's logs. Multiplexing log lines onto the shared stream would
/// send every open tab the output of every followed container.
async fn follow(
    principal: Authorized<perm::LogsView>,
    State(state): State<AppState>,
    Path((host_id, id)): Path<(i64, String)>,
    Query(query): Query<FollowQuery>,
) -> Result<
    axum::response::sse::Sse<
        impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
    >,
    ApiError,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures::StreamExt as _;

    let client = client_for(&state, host_id).await?;
    let revoked = state.revocations.until_revoked(&principal);

    let lines = client
        .follow_logs(&id, query.since)
        .take_while(|line| {
            if let Err(e) = line {
                tracing::warn!(error = %e, "log stream ended with an error");
            }
            std::future::ready(line.is_ok())
        })
        .filter_map(|line| async move {
            let line = line.ok()?;
            Event::default().event("line").json_data(&line).ok()
        });
    // The data must not be empty: a browser's EventSource silently discards
    // an event with no data, and the client would see only the close.
    let end = futures::stream::once(async { Event::default().event("end").data("stopped") });
    let events = lines.chain(end).map(Ok).take_until(revoked);

    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

/// Following a log over a WebSocket, which is what the browser uses: a
/// request held open would take one of the six connections a browser allows
/// per host over HTTP/1.1, shared across every tab.
///
/// Each message is a JSON log line. When the container stops the socket
/// closes normally with the reason `stopped`.
async fn follow_socket(
    principal: Authorized<perm::LogsView>,
    // Before the upgrade: a page on another origin can open a socket here.
    _origin: crate::origin::SameOrigin,
    State(state): State<AppState>,
    Path((host_id, id)): Path<(i64, String)>,
    Query(query): Query<FollowQuery>,
    upgrade: axum::extract::ws::WebSocketUpgrade,
) -> Result<axum::response::Response, ApiError> {
    let client = client_for(&state, host_id).await?.clone();
    let revoked = state.revocations.until_revoked(&principal);
    let lines = client.follow_logs(&id, query.since);
    Ok(upgrade.on_upgrade(move |socket| pump_lines(socket, lines, revoked)))
}

async fn pump_lines(
    socket: axum::extract::ws::WebSocket,
    lines: impl futures::Stream<Item = docker::Result<shared::logs::LogLine>> + Send + 'static,
    revoked: impl std::future::Future<Output = ()> + Send + 'static,
) {
    use axum::extract::ws::{CloseFrame, Message, close_code};
    use futures::{SinkExt as _, StreamExt as _};

    const PING: std::time::Duration = std::time::Duration::from_secs(20);
    let close = |code, reason: &'static str| {
        Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        }))
    };

    let (mut sink, mut incoming) = socket.split();
    let mut lines = Box::pin(lines);
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING, PING);
    tokio::pin!(revoked);
    loop {
        tokio::select! {
            () = &mut revoked => {
                let _ = sink.send(close(close_code::POLICY, "revoked")).await;
                return;
            }
            line = lines.next() => match line {
                Some(Ok(line)) => {
                    let Ok(json) = serde_json::to_string(&line) else { continue };
                    if sink.send(Message::Text(json.into())).await.is_err() {
                        return;
                    }
                }
                Some(Err(e)) => {
                    tracing::warn!(error = %e, "log stream ended with an error");
                    let _ = sink.send(close(close_code::ERROR, "failed")).await;
                    return;
                }
                None => {
                    let _ = sink.send(close(close_code::NORMAL, "stopped")).await;
                    return;
                }
            },
            _ = ping.tick() => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return;
                }
            }
            message = incoming.next() => match message {
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            },
        }
    }
}

/// The same output as plain text, for saving.
///
/// A download served by the server rather than assembled in the browser: a
/// link with `download` works everywhere, including on a phone, without
/// building a blob in wasm.
async fn logs_text(
    _principal: Authorized<perm::LogsView>,
    State(state): State<AppState>,
    Path((host_id, id)): Path<(i64, String)>,
    Query(query): Query<LogQuery>,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse as _;

    let client = client_for(&state, host_id).await?;
    let tail = query.tail.unwrap_or(MAX_TAIL).clamp(1, MAX_TAIL);
    let logs = client.container_logs(&id, tail).await?;

    let body: String = logs
        .lines
        .iter()
        .map(|line| match &line.at {
            Some(at) => format!("{at} {}\n", line.text),
            None => format!("{}\n", line.text),
        })
        .collect();

    let short = &id[..id.len().min(12)];
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8".to_owned(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{short}.log\""),
            ),
        ],
        body,
    )
        .into_response())
}

async fn preview(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
) -> Result<Json<CleanupPreview>, ApiError> {
    let client = client_for(&state, host_id).await?;
    Ok(Json(
        client.cleanup_preview(&registered(&state).await?).await?,
    ))
}

/// Compose projects GhostDock manages. Their stopped containers are kept.
async fn registered(state: &AppState) -> Result<std::collections::HashSet<String>, ApiError> {
    Ok(state
        .store
        .stacks_list(store::hosts::LOCAL_HOST_ID)
        .await?
        .into_iter()
        .map(|s| s.slug)
        .collect())
}

/// Removes images, having shown what they are first.
///
/// The scope is required rather than defaulted: "remove everything unused"
/// and "remove untagged layers" are different decisions, and guessing which
/// one someone meant is not a kindness.
async fn run_cleanup(
    principal: Authorized<perm::CleanupRun>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
    Json(request): Json<CleanupRequest>,
) -> Result<Json<CleanupResult>, ApiError> {
    let principal = principal.clone();
    // Detached: removal abandoned halfway leaves the host neither as it was
    // nor as the preview promised.
    crate::detach::finish(async move { clean(state, principal, host_id, request).await })
        .await
        .map(Json)
}

async fn clean(
    state: AppState,
    principal: crate::auth::Principal,
    host_id: i64,
    request: CleanupRequest,
) -> Result<CleanupResult, ApiError> {
    let client = client_for(&state, host_id).await?.clone();

    // Re-read rather than trusting a list the caller sends: the preview may
    // be minutes old, and acting on it could remove an image that has since
    // been put to use.
    let preview = client.cleanup_preview(&registered(&state).await?).await?;
    let result = match request.scope {
        CleanupScope::Dangling => client.remove_images(&preview.dangling).await,
        CleanupScope::AllUnused => {
            let all: Vec<_> = preview.dangling.into_iter().chain(preview.unused).collect();
            client.remove_images(&all).await
        }
        CleanupScope::Leftover => client.remove_containers(&preview.leftover).await,
        CleanupScope::Standalone => client.remove_containers(&preview.standalone).await,
    };

    let (action, target, detail) = match request.scope {
        CleanupScope::Dangling | CleanupScope::AllUnused => (
            "remove images",
            format!("{} images", result.removed.len()),
            format!("{} bytes reclaimed", result.reclaimed_bytes),
        ),
        CleanupScope::Leftover | CleanupScope::Standalone => (
            "remove containers",
            format!("{} containers", result.removed.len()),
            result.removed.join(", "),
        ),
    };
    crate::audit::record(&state, &principal, action, &target, Some(&detail)).await;
    Ok(result)
}

async fn client_for(state: &AppState, host_id: i64) -> Result<&docker::Client, ApiError> {
    state
        .store
        .host_by_id(host_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    state.docker.as_ref().ok_or(ApiError::DaemonUnavailable)
}
