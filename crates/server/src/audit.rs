//! Recording what was done, and showing it.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use shared::audit::AuditEntry;

use crate::auth::{Authorized, Principal, perm};
use crate::error::ApiError;
use crate::state::AppState;

/// How many entries the activity view returns.
const RECENT: i64 = 200;

pub fn routes() -> Router<AppState> {
    Router::new().route("/audit", get(recent))
}

async fn recent(
    _principal: Authorized<perm::ActivityView>,
    State(state): State<AppState>,
) -> Result<Json<Vec<AuditEntry>>, ApiError> {
    Ok(Json(state.store.audit_recent(RECENT).await?))
}

/// Records an action, never failing the request it describes.
///
/// A deploy that worked must not be reported as failed because the trail
/// could not be written; the failure is logged instead, where it is a
/// problem for the operator rather than for the person who pressed the
/// button.
pub async fn record(
    state: &AppState,
    principal: &Principal,
    action: &str,
    target: &str,
    detail: Option<&str>,
) {
    if let Err(e) = state
        .store
        .audit(
            Some(principal.user.id),
            &principal.display_name(),
            action,
            target,
            detail,
        )
        .await
    {
        tracing::error!(error = %e, action, target, "could not record an action");
    }
}

/// Records an action taken before anyone is signed in.
pub async fn record_anonymous(state: &AppState, username: &str, action: &str, target: &str) {
    if let Err(e) = state
        .store
        .audit(None, username, action, target, None)
        .await
    {
        tracing::error!(error = %e, action, "could not record an action");
    }
}
