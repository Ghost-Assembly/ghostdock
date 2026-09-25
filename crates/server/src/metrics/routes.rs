//! Resource figures over HTTP.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use shared::metrics::{MAX_POINTS, Now, Range, Series, Target};

use crate::auth::{Authorized, perm};
use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/hosts/{host_id}/metrics/now", get(now))
        .route("/hosts/{host_id}/metrics/series", get(series))
        .route("/hosts/{host_id}/metrics/containers", get(containers))
        .route("/hosts/{host_id}/sizing", get(sizing))
}

async fn now(
    _p: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
) -> Result<Json<Now>, ApiError> {
    crate::hosts::known(&state, host_id).await?;
    Ok(Json(Now::clone(&state.sampler.now())))
}

#[derive(Debug, Deserialize)]
pub struct SeriesQuery {
    subject: Option<String>,
    range: Option<String>,
}

async fn series(
    _p: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
    Query(q): Query<SeriesQuery>,
) -> Result<Json<Series>, ApiError> {
    crate::hosts::known(&state, host_id).await?;
    let text = q.subject.ok_or_else(|| ApiError::BadRequest("Say which subject: host, container:<name>, disk:<path>, network:<name> or stack:<project>.".to_owned()))?;
    let target = Target::parse(&text)
        .ok_or_else(|| ApiError::BadRequest(format!("Not a subject: {text}.")))?;
    let range = q.range.as_deref().and_then(Range::parse).ok_or_else(|| {
        ApiError::BadRequest("The range is one of 1h, 24h, 7d, 30d or 1y.".to_owned())
    })?;
    let points = crate::metrics::read_series(&state.sampler, &target, range).await?;
    Ok(Json(Series {
        target: target.to_param(),
        range,
        step: range.resolution().step(),
        points: domain::metrics::thin(&points, MAX_POINTS),
    }))
}

async fn sizing(
    _p: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
) -> Result<Json<Vec<shared::metrics::Recommendation>>, ApiError> {
    crate::hosts::known(&state, host_id).await?;
    Ok(Json(state.sampler.sizing().await?))
}

#[derive(Debug, Deserialize)]
pub struct ContainersQuery {
    project: Option<String>,
    range: Option<String>,
}

/// Each of a stack's containers over a range: typical and peak figures.
async fn containers(
    _p: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
    Query(q): Query<ContainersQuery>,
) -> Result<Json<Vec<shared::metrics::ContainerFigures>>, ApiError> {
    crate::hosts::known(&state, host_id).await?;
    let project = q
        .project
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| ApiError::BadRequest("Say which stack: project=<name>.".to_owned()))?;
    let range = q.range.as_deref().and_then(Range::parse).ok_or_else(|| {
        ApiError::BadRequest("The range is one of 1h, 24h, 7d, 30d or 1y.".to_owned())
    })?;
    Ok(Json(
        crate::metrics::container_figures(&state.sampler, &project, range).await?,
    ))
}
