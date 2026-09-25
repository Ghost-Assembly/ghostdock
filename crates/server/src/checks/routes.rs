//! Uptime checks over HTTP.

use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::Utc;
use shared::checks::{
    Check, CheckHistory, CheckInput, CheckState, CheckStatus, CheckSummary, Incident,
};
use shared::metrics::{MAX_POINTS, Range, Resolution};
use shared::reference::Access;
use shared::token::Permission;

use crate::auth::{Authorized, perm};
use crate::error::ApiError;
use crate::reference::Routes;
use crate::state::AppState;

/// Latencies in a list's sparkline.
const RECENT: u32 = 30;
/// Incidents in the host's list.
const INCIDENTS: i64 = 100;
const DAY: i64 = 86_400;

pub fn routes() -> Routes {
    let view = Access::Token(Permission::HostView);
    let manage = Access::Token(Permission::ChecksManage);
    Routes::new("Uptime checks")
        .get(
            "/hosts/{host_id}/checks",
            view,
            "Every uptime check with its state, uptime over 24 hours and 30 days, and latest latencies",
            list,
        )
        .post(
            "/hosts/{host_id}/checks",
            manage,
            "Adds an uptime check: an http(s) URL, a tcp host:port, or a container by name",
            create,
        )
        .get(
            "/hosts/{host_id}/incidents",
            view,
            "The latest 100 times a check went down, newest first",
            incidents,
        )
        .get("/checks/{id}", view, "One uptime check with its state and uptime", one)
        .put(
            "/checks/{id}",
            manage,
            "Changes an uptime check; only the settings sent change, and it runs again soon",
            update,
        )
        .delete(
            "/checks/{id}",
            manage,
            "Removes an uptime check with its history and incidents",
            remove,
        )
        .get(
            "/checks/{id}/history",
            view,
            "A check's runs over ?range= (1h, 24h, 7d, 30d or 1y): latency, uptime and incidents",
            history,
        )
}

async fn list(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
) -> Result<Json<Vec<CheckSummary>>, ApiError> {
    crate::hosts::known(&state, host_id).await?;
    let checks = state.store.checks_list(host_id).await?;
    Ok(Json(summarise(&state, checks).await?))
}

async fn one(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<CheckSummary>, ApiError> {
    let check = state
        .store
        .check_by_id(id)
        .await?
        .ok_or(ApiError::NotFound)?;
    summary(&state, check).await.map(Json)
}

async fn create(
    principal: Authorized<perm::ChecksManage>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
    Json(input): Json<CheckInput>,
) -> Result<Json<CheckSummary>, ApiError> {
    crate::hosts::known(&state, host_id).await?;
    let check = domain::checks::settle(&input, None).map_err(ApiError::BadRequest)?;
    stack_exists(&state, check.stack_id).await?;
    let made = state
        .store
        .check_create(host_id, &check)
        .await
        .map_err(|e| taken(e, &check.name))?;
    state.checks.restart(made.id).await;
    // What is checked, never where: a URL can carry a token.
    crate::audit::record(
        &state,
        &principal,
        "add check",
        &made.name,
        Some(made.kind.as_str()),
    )
    .await;
    summary(&state, made).await.map(Json)
}

async fn update(
    principal: Authorized<perm::ChecksManage>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<CheckInput>,
) -> Result<Json<CheckSummary>, ApiError> {
    let current = state
        .store
        .check_by_id(id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let check = domain::checks::settle(&input, Some(&current)).map_err(ApiError::BadRequest)?;
    stack_exists(&state, check.stack_id).await?;
    let saved = state
        .store
        .check_update(&check)
        .await
        .map_err(|e| taken(e, &check.name))?;
    state.checks.restart(id).await;
    crate::audit::record(&state, &principal, "change check", &saved.name, None).await;
    summary(&state, saved).await.map(Json)
}

async fn remove(
    principal: Authorized<perm::ChecksManage>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<axum::http::StatusCode, ApiError> {
    let check = state
        .store
        .check_by_id(id)
        .await?
        .ok_or(ApiError::NotFound)?;
    state.store.check_delete(id).await?;
    state.checks.forget(id);
    if let Some(metrics) = state.sampler.store()
        && let Err(e) = metrics.forget_check(id).await
    {
        tracing::warn!(error = %e, check = id, "could not remove a check's history");
    }
    crate::audit::record(&state, &principal, "remove check", &check.name, None).await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn incidents(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
) -> Result<Json<Vec<Incident>>, ApiError> {
    crate::hosts::known(&state, host_id).await?;
    Ok(Json(
        state.store.incidents_recent(host_id, INCIDENTS).await?,
    ))
}

#[derive(Debug, serde::Deserialize)]
struct HistoryQuery {
    range: Option<String>,
}

async fn history(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<CheckHistory>, ApiError> {
    let check = state
        .store
        .check_by_id(id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let range = match query.range.as_deref() {
        None => Range::Day,
        Some(text) => Range::parse(text).ok_or_else(|| {
            ApiError::BadRequest("The range is 1h, 24h, 7d, 30d or 1y.".to_owned())
        })?,
    };
    // Runs are kept a week; longer comes from the hours.
    let resolution = match range {
        Range::Month | Range::Year => Resolution::Quarter,
        _ => Resolution::Minute,
    };
    let to = Utc::now().timestamp() + 1;
    let from = to - range.seconds();
    let points = match state.sampler.store() {
        Some(metrics) => {
            metrics
                .check_series(id, resolution, from, to, MAX_POINTS)
                .await?
        }
        None => Vec::new(),
    };
    let step = store::metrics::check_step(resolution, from, to, MAX_POINTS);
    // Points closer together than the check runs are not a gap.
    let step = if resolution == Resolution::Quarter {
        step
    } else {
        step.max(i64::from(check.interval_s))
    };
    let (up, total) = points.iter().fold((0_u64, 0_u64), |(u, t), p| {
        (u + u64::from(p.up), t + u64::from(p.total))
    });
    Ok(Json(CheckHistory {
        check_id: id,
        range,
        step,
        points,
        uptime: share(up, total),
        incidents: state.store.incidents_for(id, from).await?,
    }))
}

/// Refuses a stack that does not exist, rather than failing on the key.
async fn stack_exists(state: &AppState, stack_id: Option<i64>) -> Result<(), ApiError> {
    match stack_id {
        Some(id) if state.store.stack_by_id(id).await?.is_none() => Err(ApiError::BadRequest(
            "That stack does not exist.".to_owned(),
        )),
        _ => Ok(()),
    }
}

fn taken(e: store::Error, name: &str) -> ApiError {
    match e {
        store::Error::NameTaken => {
            ApiError::Conflict(format!("A check named {name} already exists."))
        }
        store::Error::NotFound => ApiError::NotFound,
        other => ApiError::from(other),
    }
}

#[allow(clippy::cast_precision_loss)]
fn share(up: u64, total: u64) -> Option<f64> {
    (total > 0).then(|| up as f64 / total as f64)
}

async fn summary(state: &AppState, check: Check) -> Result<CheckSummary, ApiError> {
    let mut all = summarise(state, vec![check]).await?;
    all.pop().ok_or(ApiError::NotFound)
}

/// Each check with its state now, its uptime and its latest latencies.
async fn summarise(state: &AppState, checks: Vec<Check>) -> Result<Vec<CheckSummary>, ApiError> {
    let now = Utc::now().timestamp();
    let (day, month, mut recent) = match state.sampler.store() {
        Some(metrics) => (
            metrics.check_uptime(now - DAY).await?,
            metrics.check_uptime_hourly(now - 30 * DAY).await?,
            metrics.check_recent(RECENT, now - DAY).await?,
        ),
        None => Default::default(),
    };
    let of = |counts: Option<&(u32, u32)>| {
        counts.and_then(|(up, total)| share(u64::from(*up), u64::from(*total)))
    };
    Ok(checks
        .into_iter()
        .map(|check| {
            let status = state.checks.status(check.id).unwrap_or(CheckStatus {
                check_id: check.id,
                state: if check.enabled {
                    CheckState::Pending
                } else {
                    CheckState::Paused
                },
                since: None,
                last_at: None,
                latency_ms: None,
                message: None,
                tls_days_left: None,
            });
            let uptime_24h = of(day.get(&check.id));
            CheckSummary {
                status,
                uptime_24h,
                // The hours lag the runs by up to a minute.
                uptime_30d: of(month.get(&check.id)).or(uptime_24h),
                recent: recent.remove(&check.id).unwrap_or_default(),
                check,
            }
        })
        .collect())
}
