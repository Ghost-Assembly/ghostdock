//! Many containers' output at once: a snapshot merged by time, the same as
//! a download, and a WebSocket that follows them all.
//!
//! Which containers is one of `all` (every running one), `stack=<id>`
//! (every container of a registered stack) or `containers=<a,b,c>`, and
//! never more than [`MAX_CONTAINERS`]: each one followed holds a connection
//! to the daemon open.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Json;
use axum::extract::ws::{Message, WebSocketUpgrade, close_code};
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse as _, Response};
use domain::logs::{MAX_CONTAINERS, Selection};
use futures::StreamExt as _;
use serde::Deserialize;
use shared::event::ContainerChange;
use shared::logs::{LiveLog, MergedLogs, TaggedLine};
use shared::reference::Access;
use shared::token::Permission;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::auth::{Authorized, perm};
use crate::error::ApiError;
use crate::reference::Routes;
use crate::socket::{Next, Source, close};
use crate::state::AppState;

/// Lines read from each container when the caller does not say.
const DEFAULT_TAIL: usize = 100;
/// Most read from each container however many are asked for, and what the
/// download reads.
const MAX_TAIL: usize = 1_000;
/// Most lines in one answer, newest kept: fifty containers' tails merged
/// would otherwise be tens of thousands of lines for a phone.
const MAX_LINES: usize = 5_000;
/// Lines waiting to be sent before more are dropped and counted. A slow
/// reader then misses lines rather than making the server hold them all.
const QUEUE: usize = 1_024;

#[derive(Debug, Deserialize)]
pub struct AcrossQuery {
    all: Option<String>,
    stack: Option<String>,
    containers: Option<String>,
    tail: Option<usize>,
    /// From this Unix time, in seconds.
    since: Option<i64>,
    /// Only lines containing this, ignoring case.
    contains: Option<String>,
}

impl AcrossQuery {
    fn selection(&self) -> Result<Selection, ApiError> {
        Selection::parse(
            self.all.as_deref(),
            self.stack.as_deref(),
            self.containers.as_deref(),
        )
        .map_err(ApiError::BadRequest)
    }

    /// The text to look for, lower-cased once, or none.
    fn needle(&self) -> Option<Arc<str>> {
        self.contains
            .as_deref()
            .filter(|c| !c.is_empty())
            .map(|c| c.to_lowercase().into())
    }
}

pub fn routes() -> Routes {
    let logs_view = Access::Token(Permission::LogsView);
    Routes::new("Logs")
        .get(
            "/hosts/{host_id}/logs",
            logs_view,
            "Several containers' output merged by time: ?all (running ones), ?stack=<id> or ?containers=<a,b>, at most 50; ?tail=<lines each> (default 100, at most 1000), ?since=<unix seconds>, ?contains=<text>",
            snapshot,
        )
        .get(
            "/hosts/{host_id}/logs.txt",
            logs_view,
            "The same merged output as a plain-text download",
            snapshot_text,
        )
        .get(
            "/hosts/{host_id}/logs/socket",
            logs_view,
            "The same containers followed over one WebSocket: a JSON line a message, or {\"skipped\":n} when lines were dropped for a slow reader. all and stack pick up containers as they start",
            follow_socket,
        )
}

/// A container being read: how the daemon knows it, and how its lines are
/// labelled.
#[derive(Debug, Clone)]
struct Target {
    id: String,
    name: String,
    stack: Option<String>,
}

/// Which containers started later belong to a followed selection.
#[derive(Debug, Clone)]
enum Scope {
    All,
    Project(String),
}

/// The containers `selection` names now, and for `all` and `stack`, the
/// scope that containers starting later are matched against.
async fn resolve(
    state: &AppState,
    client: &docker::Client,
    host_id: i64,
    selection: &Selection,
) -> Result<(Vec<Target>, Option<Scope>), ApiError> {
    let target = |c: shared::container::Container| Target {
        id: c.id,
        name: c.name,
        stack: c.compose.map(|m| m.project),
    };
    let listed = client.list_containers().await?;
    let (targets, scope) = match selection {
        Selection::All => (
            listed
                .into_iter()
                .filter(|c| c.state.is_running())
                .map(target)
                .collect::<Vec<_>>(),
            Some(Scope::All),
        ),
        Selection::Stack(id) => {
            let stack = state
                .store
                .stack_by_id(*id)
                .await?
                .filter(|s| s.host_id == host_id)
                .ok_or(ApiError::NotFound)?;
            let targets = listed
                .into_iter()
                .filter(|c| c.compose.as_ref().is_some_and(|m| m.project == stack.slug))
                .map(target)
                .collect();
            (targets, Some(Scope::Project(stack.slug)))
        }
        Selection::Containers(names) => {
            let mut targets: Vec<Target> = Vec::new();
            for name in names {
                let found = listed.iter().find(|c| {
                    c.name == *name || c.id == *name || (name.len() >= 12 && c.id.starts_with(name))
                });
                let Some(found) = found else {
                    return Err(ApiError::BadRequest(format!(
                        "No container is called {name}."
                    )));
                };
                if !targets.iter().any(|t| t.id == found.id) {
                    targets.push(target(found.clone()));
                }
            }
            (targets, None)
        }
    };
    domain::logs::check_count(targets.len()).map_err(ApiError::BadRequest)?;
    Ok((targets, scope))
}

/// Each target's latest lines, merged.
async fn read(
    client: &docker::Client,
    targets: &[Target],
    tail: usize,
    query: &AcrossQuery,
) -> Result<MergedLogs, ApiError> {
    let reads = targets
        .iter()
        .map(|t| client.container_logs_since(&t.id, tail, query.since));
    let needle = query.needle();
    let mut truncated = false;
    let mut sources = Vec::with_capacity(targets.len());
    for (target, read) in targets.iter().zip(futures::future::join_all(reads).await) {
        let logs = match read {
            Ok(logs) => logs,
            // Removed since it was listed: it has nothing more to say.
            Err(e) if e.kind() == docker::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        truncated |= logs.truncated;
        sources.push(
            logs.lines
                .into_iter()
                .filter(|l| {
                    needle
                        .as_deref()
                        .is_none_or(|n| l.text.to_lowercase().contains(n))
                })
                .map(|l| tag(target, l))
                .collect(),
        );
    }
    let (lines, cut) = domain::logs::merge(sources, MAX_LINES);
    Ok(MergedLogs {
        containers: targets.iter().map(|t| t.name.clone()).collect(),
        lines,
        truncated: truncated || cut,
    })
}

fn tag(target: &Target, line: shared::logs::LogLine) -> TaggedLine {
    TaggedLine {
        container: target.name.clone(),
        stack: target.stack.clone(),
        stream: line.stream,
        at: line.at,
        text: line.text,
    }
}

async fn snapshot(
    _principal: Authorized<perm::LogsView>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
    Query(query): Query<AcrossQuery>,
) -> Result<Json<MergedLogs>, ApiError> {
    let selection = query.selection()?;
    let client = crate::hosts::daemon(&state, host_id).await?;
    let (targets, _) = resolve(&state, client, host_id, &selection).await?;
    let tail = query.tail.unwrap_or(DEFAULT_TAIL).clamp(1, MAX_TAIL);
    Ok(Json(read(client, &targets, tail, &query).await?))
}

/// The same as plain text, for saving: a link with `download` works on a
/// phone without assembling a file in the browser.
async fn snapshot_text(
    _principal: Authorized<perm::LogsView>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
    Query(query): Query<AcrossQuery>,
) -> Result<Response, ApiError> {
    let selection = query.selection()?;
    let client = crate::hosts::daemon(&state, host_id).await?;
    let (targets, _) = resolve(&state, client, host_id, &selection).await?;
    let tail = query.tail.unwrap_or(MAX_TAIL).clamp(1, MAX_TAIL);
    let logs = read(client, &targets, tail, &query).await?;

    let body: String = logs
        .lines
        .iter()
        .map(|line| match &line.at {
            Some(at) => format!("{at} {} {}\n", line.container, line.text),
            None => format!("{} {}\n", line.container, line.text),
        })
        .collect();
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8",
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"containers.log\"",
            ),
        ],
        body,
    )
        .into_response())
}

/// Following over a WebSocket, which is what the browser uses: a request
/// held open would take one of the six connections a browser allows per
/// host over HTTP/1.1, shared across every tab.
///
/// For an explicit list, the socket closes normally with the reason
/// `stopped` once every container in it has stopped.
async fn follow_socket(
    principal: Authorized<perm::LogsView>,
    // Before the upgrade: a page on another origin can open a socket here.
    _origin: crate::origin::SameOrigin,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
    Query(query): Query<AcrossQuery>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let selection = query.selection()?;
    let client = crate::hosts::daemon(&state, host_id).await?.clone();
    let (targets, scope) = resolve(&state, &client, host_id, &selection).await?;
    let revoked = state.revocations.until_revoked(&principal);
    let fanin = Fanin::start(client, targets, scope, &query);
    Ok(upgrade.on_upgrade(move |socket| crate::socket::pump(socket, fanin, revoked)))
}

type Changes = Pin<Box<dyn futures::Stream<Item = docker::Result<ContainerChange>> + Send>>;

/// One follow task per container, all feeding one bounded queue.
///
/// Dropping it, when the socket ends, aborts every task.
struct Fanin {
    client: docker::Client,
    tx: mpsc::Sender<TaggedLine>,
    lines: mpsc::Receiver<TaggedLine>,
    /// Lines dropped since the reader was last told.
    skipped: Arc<AtomicU64>,
    needle: Option<Arc<str>>,
    /// Each task ends with the container it followed and its generation.
    followers: JoinSet<(String, u64)>,
    /// Containers followed now, by id, with the generation of their task:
    /// a container that restarts gets a new task, and the old one ending
    /// must not remove the new one.
    following: HashMap<String, u64>,
    generation: u64,
    /// For `all` and `stack`: containers that start are followed too.
    watch: Option<(Scope, Changes)>,
}

impl Fanin {
    fn start(
        client: docker::Client,
        targets: Vec<Target>,
        scope: Option<Scope>,
        query: &AcrossQuery,
    ) -> Self {
        let (tx, lines) = mpsc::channel(QUEUE);
        // Subscribed when first polled, which the socket does at once.
        let watch = scope.map(|scope| {
            let changes: Changes = Box::pin(client.container_changes());
            (scope, changes)
        });
        let mut fanin = Self {
            client,
            tx,
            lines,
            skipped: Arc::new(AtomicU64::new(0)),
            needle: query.needle(),
            followers: JoinSet::new(),
            following: HashMap::new(),
            generation: 0,
            watch,
        };
        for target in targets {
            fanin.follow(target, query.since);
        }
        fanin
    }

    fn follow(&mut self, target: Target, since: Option<i64>) {
        if self.following.contains_key(&target.id) {
            return;
        }
        if self.following.len() >= MAX_CONTAINERS {
            tracing::debug!(container = %target.name, "not followed: at the limit");
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        self.following.insert(target.id.clone(), generation);

        let lines = self.client.follow_logs(&target.id, since);
        let tx = self.tx.clone();
        let skipped = Arc::clone(&self.skipped);
        let needle = self.needle.clone();
        self.followers.spawn(async move {
            let mut lines = std::pin::pin!(lines);
            while let Some(line) = lines.next().await {
                let line = match line {
                    Ok(line) => line,
                    Err(e) => {
                        tracing::warn!(container = %target.name, error = %e, "a followed log ended with an error");
                        break;
                    }
                };
                if needle
                    .as_deref()
                    .is_some_and(|n| !line.text.to_lowercase().contains(n))
                {
                    continue;
                }
                match tx.try_send(tag(&target, line)) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        skipped.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                }
            }
            (target.id, generation)
        });
    }

    /// A follow task ended: its container stopped or went.
    fn ended(&mut self, done: Result<(String, u64), tokio::task::JoinError>) {
        if let Ok((id, generation)) = done
            && self.following.get(&id) == Some(&generation)
        {
            self.following.remove(&id);
        }
    }

    /// A container in scope started or stopped.
    fn changed(&mut self, change: ContainerChange) {
        let Some((scope, _)) = &self.watch else {
            return;
        };
        let in_scope = match scope {
            Scope::All => true,
            Scope::Project(project) => change.project.as_deref() == Some(project.as_str()),
        };
        if !in_scope {
            return;
        }
        match change.action.as_str() {
            "start" => {
                let name = change
                    .name
                    .unwrap_or_else(|| shared::short(&change.container_id, 12).to_owned());
                // From a second back: what it wrote while this heard about
                // it. `since` is whole seconds, so it cannot be tighter.
                let since = chrono::Utc::now().timestamp() - 1;
                self.follow(
                    Target {
                        id: change.container_id,
                        name,
                        stack: change.project,
                    },
                    Some(since),
                );
            }
            // Its task ends by itself when the daemon ends the stream,
            // after the last lines, so it is not cut short here.
            "die" | "destroy" => {
                self.following.remove(&change.container_id);
            }
            _ => {}
        }
    }
}

/// What the socket heard while waiting.
enum Heard {
    Line(TaggedLine),
    Ended(Result<(String, u64), tokio::task::JoinError>),
    Changed(Option<docker::Result<ContainerChange>>),
}

fn message(live: &LiveLog) -> Option<Next> {
    serde_json::to_string(live)
        .ok()
        .map(|json| Next::Send(Message::Text(json.into())))
}

impl Source for Fanin {
    async fn next(&mut self) -> Next {
        loop {
            let skipped = self.skipped.swap(0, Ordering::Relaxed);
            if skipped > 0
                && let Some(next) = message(&LiveLog::Skipped { skipped })
            {
                return next;
            }
            // An explicit list with every container stopped: what is left,
            // then the end.
            if self.watch.is_none() && self.followers.is_empty() {
                match self.lines.try_recv() {
                    Ok(line) => match message(&LiveLog::Line(line)) {
                        Some(next) => return next,
                        None => continue,
                    },
                    Err(_) => return Next::End(Some(close(close_code::NORMAL, "stopped"))),
                }
            }

            let Self {
                lines,
                followers,
                watch,
                ..
            } = self;
            let heard = tokio::select! {
                biased;
                Some(line) = lines.recv() => Heard::Line(line),
                Some(done) = followers.join_next(), if !followers.is_empty() => Heard::Ended(done),
                change = async {
                    match watch.as_mut() {
                        Some((_, changes)) => changes.next().await,
                        None => std::future::pending().await,
                    }
                }, if watch.is_some() => Heard::Changed(change),
            };
            match heard {
                Heard::Line(line) => {
                    if let Some(next) = message(&LiveLog::Line(line)) {
                        return next;
                    }
                }
                Heard::Ended(done) => self.ended(done),
                Heard::Changed(Some(Ok(change))) => self.changed(change),
                Heard::Changed(Some(Err(e))) => {
                    tracing::warn!(error = %e, "lost the Docker event stream while following logs");
                    return Next::End(Some(close(close_code::ERROR, "failed")));
                }
                Heard::Changed(None) => {
                    return Next::End(Some(close(close_code::ERROR, "failed")));
                }
            }
        }
    }
}
