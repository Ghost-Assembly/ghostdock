//! Host, container, and stack reads.
//!
//! Every route sits beneath a host id even though v1 manages exactly one
//! host, so adding more later is routing, not a redesign.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use shared::container::Container;
use shared::host::{Host, HostInfo};
use shared::stack::{Managed, Stack};

use crate::auth::{Authorized, perm};
use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/hosts", get(list_hosts))
        .route("/hosts/{host_id}", get(host_info))
        .route("/hosts/{host_id}/containers", get(list_containers))
        .route("/hosts/{host_id}/stacks", get(list_stacks))
}

async fn list_hosts(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
) -> Result<Json<Vec<Host>>, ApiError> {
    Ok(Json(state.store.hosts_list().await?))
}

async fn host_info(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
) -> Result<Json<HostInfo>, ApiError> {
    let host = state
        .store
        .host_by_id(host_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let client = state.docker.as_ref().ok_or(ApiError::DaemonUnavailable)?;
    let mut info = client.host_info(host.id, host.name).await;
    info.problems = state.problems.as_ref().clone();
    Ok(Json(info))
}

async fn list_containers(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
) -> Result<Json<Vec<Container>>, ApiError> {
    let client = client_for(&state, host_id).await?;
    Ok(Json(client.list_containers().await?))
}

/// Stacks as the UI renders them: what is running on the host, merged with
/// what GhostDock manages.
///
/// One list rather than two. A user thinks in terms of "my stacks", not
/// "stacks GhostDock knows about" versus "stacks that happen to exist".
async fn list_stacks(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
) -> Result<Json<Vec<Stack>>, ApiError> {
    let client = client_for(&state, host_id).await?;
    let containers = client.list_containers().await?;

    let managed = state
        .store
        .stacks_list(host_id)
        .await?
        .into_iter()
        .map(|stack| {
            let busy = state.runner.is_busy(stack.id);
            (
                stack.slug,
                Managed {
                    id: stack.id,
                    name: stack.name,
                    source_kind: stack.source_kind,
                    busy,
                },
            )
        })
        .collect();

    Ok(Json(domain::stack::merge(containers, managed).stacks))
}

/// Resolves the host then its client, so an unknown host id is a 404 rather
/// than being silently served from the one daemon we happen to have.
async fn client_for(state: &AppState, host_id: i64) -> Result<&docker::Client, ApiError> {
    state
        .store
        .host_by_id(host_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    state.docker.as_ref().ok_or(ApiError::DaemonUnavailable)
}
