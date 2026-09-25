//! Reading logs and reclaiming space.

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use shared::cleanup::{CleanupPreview, CleanupRequest, CleanupResult, CleanupScope};
use shared::logs::Logs;
use shared::reference::Access;
use shared::token::Permission;

use crate::auth::{Authorized, perm};
use crate::error::ApiError;
use crate::reference::Routes;
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

pub fn routes() -> Routes {
    let logs_view = Access::Token(Permission::LogsView);
    Routes::new("Logs")
        .get(
            "/hosts/{host_id}/containers/{id}/logs",
            logs_view,
            "A container's latest output: ?tail=<lines>, default 500, at most 5000",
            logs,
        )
        .get(
            "/hosts/{host_id}/containers/{id}/logs.txt",
            logs_view,
            "The same output as a plain-text download",
            logs_text,
        )
        .get(
            "/hosts/{host_id}/containers/{id}/logs/follow",
            logs_view,
            "New output as server-sent events: line for each line, end when the container stops",
            follow,
        )
        .get(
            "/hosts/{host_id}/containers/{id}/logs/socket",
            logs_view,
            "New output over a WebSocket, as the browser follows it",
            follow_socket,
        )
        .area("Cleanup")
        .get(
            "/hosts/{host_id}/cleanup",
            Access::Token(Permission::HostView),
            "What cleanup would remove: unused images and stray containers, with sizes",
            preview,
        )
        .post(
            "/hosts/{host_id}/cleanup",
            Access::Token(Permission::CleanupRun),
            "Removes one scope of what the preview lists, re-read at the time",
            run_cleanup,
        )
}

async fn logs(
    _principal: Authorized<perm::LogsView>,
    State(state): State<AppState>,
    Path((host_id, id)): Path<(i64, String)>,
    Query(query): Query<LogQuery>,
) -> Result<Json<Logs>, ApiError> {
    let client = crate::hosts::daemon(&state, host_id).await?;
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

    let client = crate::hosts::daemon(&state, host_id).await?;
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
    let client = crate::hosts::daemon(&state, host_id).await?.clone();
    let revoked = state.revocations.until_revoked(&principal);
    let lines = Lines(Box::pin(client.follow_logs(&id, query.since)));
    Ok(upgrade.on_upgrade(move |socket| crate::socket::pump(socket, lines, revoked)))
}

/// One container's output, a JSON line a message.
struct Lines<S>(std::pin::Pin<Box<S>>);

impl<S> crate::socket::Source for Lines<S>
where
    S: futures::Stream<Item = docker::Result<shared::logs::LogLine>> + Send,
{
    async fn next(&mut self) -> crate::socket::Next {
        use crate::socket::{Next, close};
        use axum::extract::ws::{Message, close_code};
        use futures::StreamExt as _;

        loop {
            match self.0.next().await {
                Some(Ok(line)) => {
                    if let Ok(json) = serde_json::to_string(&line) {
                        return Next::Send(Message::Text(json.into()));
                    }
                }
                Some(Err(e)) => {
                    tracing::warn!(error = %e, "log stream ended with an error");
                    return Next::End(Some(close(close_code::ERROR, "failed")));
                }
                None => return Next::End(Some(close(close_code::NORMAL, "stopped"))),
            }
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

    let client = crate::hosts::daemon(&state, host_id).await?;
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

    // By characters: a byte slice of a name could land inside one and panic.
    let short = shared::short(&id, 12);
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
    let client = crate::hosts::daemon(&state, host_id).await?;
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
    let client = crate::hosts::daemon(&state, host_id).await?;

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
