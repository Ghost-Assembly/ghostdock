//! What is waiting, and whether to apply it without being asked.

use axum::extract::{Path, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use shared::update::{AutoApply, StackUpdate, UpdateStatus};

use crate::auth::{Authorized, perm};
use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/hosts/{host_id}/updates", get(list))
        .route("/stacks/{id}/check", post(check_now))
        .route("/stacks/{id}/auto-apply", put(set_auto_apply))
}

/// Every stack with what is waiting for it.
///
/// Returns all of them, not only those with something pending: a screen that
/// hides the current ones gives no way to tell "nothing is waiting" from
/// "nothing has been checked".
async fn list(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
) -> Result<Json<Vec<StackUpdate>>, ApiError> {
    let stacks = state.store.stacks_list(host_id).await?;
    let mut statuses = state.store.update_statuses(host_id).await?;

    let out = stacks
        .into_iter()
        .map(|stack| {
            let (status, auto_apply) = statuses.remove(&stack.id).unwrap_or_default();
            StackUpdate {
                reason: domain::update::reason(&status),
                auto_apply,
                busy: state.runner.is_busy(stack.id),
                status,
                stack,
            }
        })
        .collect();
    Ok(Json(out))
}

/// Checks one stack immediately.
///
/// The sweep is hourly, which is right for a background task and far too
/// slow for someone who has just pushed a commit and wants to see it.
async fn check_now(
    _principal: Authorized<perm::UpdatesCheck>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<UpdateStatus>, ApiError> {
    let stack = state
        .store
        .stack_by_id(id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(state.checker.check(&stack).await))
}

async fn set_auto_apply(
    principal: Authorized<perm::UpdatesAutoApply>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<AutoApply>,
) -> Result<Json<AutoApply>, ApiError> {
    let stack = state
        .store
        .stack_by_id(id)
        .await?
        .ok_or(ApiError::NotFound)?;
    state.store.stack_set_auto_apply(id, body.enabled).await?;
    let action = if body.enabled {
        "turn on auto-apply"
    } else {
        "turn off auto-apply"
    };
    crate::audit::record(&state, &principal, action, &stack.slug, None).await;
    Ok(Json(body))
}
