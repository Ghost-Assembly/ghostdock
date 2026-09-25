//! Host, container, and stack reads.
//!
//! Every route sits beneath a host id even though v1 manages exactly one
//! host, so adding more later is routing, not a redesign.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use domain::icon::Service;
use shared::container::Container;
use shared::host::{Host, HostInfo};
use shared::reference::Access;
use shared::stack::{Managed, Stack};
use shared::token::Permission;

use crate::auth::{Authorized, perm};
use crate::error::ApiError;
use crate::reference::Routes;
use crate::state::AppState;

pub fn routes() -> Routes {
    let view = Access::Token(Permission::HostView);
    Routes::new("Hosts")
        .get("/hosts", view, "The hosts GhostDock manages", list_hosts)
        .get(
            "/hosts/{host_id}",
            view,
            "The Docker daemon's version and counts, and any deployment problem",
            host_info,
        )
        .get(
            "/hosts/{host_id}/containers",
            view,
            "Every container on the host",
            list_containers,
        )
        .area("Stacks")
        .get(
            "/hosts/{host_id}/stacks",
            view,
            "Every compose stack on the host with its state, managed or not",
            list_stacks,
        )
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
    let host = known(&state, host_id).await?;
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
    let client = daemon(&state, host_id).await?;
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
    let client = daemon(&state, host_id).await?;
    let mut labels = HashMap::new();
    let containers = client
        .list_labeled()
        .await?
        .into_iter()
        .map(|listed| {
            labels.insert(listed.container.id.clone(), listed.labels);
            listed.container
        })
        .collect();

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

    let mut stacks = domain::stack::merge(containers, managed).stacks;
    for stack in &mut stacks {
        let services = stack.containers.iter().map(|c| Service {
            image: &c.image,
            labels: labels.get(&c.id),
        });
        stack.icon = domain::icon::for_stack(services, &stack.project).map(str::to_owned);
    }
    Ok(Json(stacks))
}

/// The host `host_id` names, or a 404.
pub(crate) async fn known(state: &AppState, host_id: i64) -> Result<Host, ApiError> {
    state
        .store
        .host_by_id(host_id)
        .await?
        .ok_or(ApiError::NotFound)
}

/// Resolves the host then its daemon, so an unknown host id is a 404 rather
/// than being silently served from the one daemon we happen to have.
pub(crate) async fn daemon(state: &AppState, host_id: i64) -> Result<&docker::Client, ApiError> {
    known(state, host_id).await?;
    state.docker.as_ref().ok_or(ApiError::DaemonUnavailable)
}
