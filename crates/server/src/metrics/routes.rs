//! Resource figures over HTTP.

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use shared::metrics::{MAX_POINTS, Now, Range, Series, Target};
use shared::reference::Access;
use shared::token::Permission;

use crate::auth::{Authorized, perm};
use crate::error::ApiError;
use crate::reference::Routes;
use crate::state::AppState;

pub fn routes() -> Routes {
    let view = Access::Token(Permission::HostView);
    Routes::new("Resources")
        .get(
            "/hosts/{host_id}/metrics/now",
            view,
            "The latest CPU, memory, network and disk figures for the host and each container",
            now,
        )
        .get(
            "/hosts/{host_id}/metrics/series",
            view,
            "One subject's figures over a range: ?subject=host|container:<name>|disk:<path>|network:<name>|stack:<project>&range=1h|24h|7d|30d|1y",
            series,
        )
        .get(
            "/hosts/{host_id}/metrics/containers",
            view,
            "Typical and peak figures for each of a stack's containers: ?project=<name>&range=…",
            containers,
        )
        .get(
            "/hosts/{host_id}/sizing",
            view,
            "CPU and memory limits each container's history supports, with the evidence",
            sizing,
        )
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
