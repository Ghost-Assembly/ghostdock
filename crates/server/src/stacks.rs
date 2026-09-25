//! Managing stacks: registering them, changing them, and running operations.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use shared::deployment::{
    Action, Deployment, DeploymentDetail, NewStack, RegisteredStack, StackCompose, Trigger,
};

use crate::auth::{Authorized, Principal, perm};
use crate::error::ApiError;
use crate::runner::RunError;
use crate::state::AppState;

/// How many past attempts a stack's history returns.
const HISTORY_LIMIT: i64 = 25;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/hosts/{host_id}/stacks", post(create))
        .route("/stacks/{id}", get(detail).put(update).delete(remove))
        .route("/stacks/{id}/compose", get(compose_file))
        .route("/stacks/{id}/deploy", post(deploy))
        .route("/stacks/{id}/stop", post(stop))
        .route("/stacks/{id}/restart", post(restart))
        .route("/stacks/{id}/down", post(down))
        .route("/stacks/{id}/deployments", get(history))
        .route("/deployments/{id}", get(deployment))
}

async fn create(
    principal: Authorized<perm::StacksCreate>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
    Json(new): Json<NewStack>,
) -> Result<Json<RegisteredStack>, ApiError> {
    crate::hosts::known(&state, host_id).await?;

    let name = new.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("Give the stack a name.".to_owned()));
    }
    if new.compose_yaml.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "The compose file is empty.".to_owned(),
        ));
    }

    // The slug becomes a Compose project name and a directory, so it is
    // derived and validated rather than taken from the user verbatim.
    let slug = compose::slug::from_name(name).ok_or_else(|| {
        ApiError::BadRequest(
            "That name has no letters or digits to build an identifier from.".to_owned(),
        )
    })?;

    let created = state
        .store
        .stack_create(host_id, &slug, name, &new.compose_yaml)
        .await
        .map_err(|e| match e {
            store::Error::SlugTaken => ApiError::Conflict(format!(
                "A stack named {slug} already exists. Compose projects must be unique."
            )),
            other => ApiError::from(other),
        })?;

    crate::audit::record(&state, &principal, "register stack", &created.slug, None).await;
    Ok(Json(created))
}

async fn load(state: &AppState, id: i64) -> Result<RegisteredStack, ApiError> {
    state.store.stack_by_id(id).await?.ok_or(ApiError::NotFound)
}

async fn detail(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<RegisteredStack>, ApiError> {
    load(&state, id).await.map(Json)
}

/// The stack's compose file, for editing it.
async fn compose_file(
    _principal: Authorized<perm::ComposeRead>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<StackCompose>, ApiError> {
    state
        .store
        .stack_compose_yaml(id)
        .await?
        .map(|compose_yaml| Json(StackCompose { compose_yaml }))
        .ok_or(ApiError::NotFound)
}

async fn update(
    principal: Authorized<perm::StacksEdit>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(new): Json<NewStack>,
) -> Result<Json<RegisteredStack>, ApiError> {
    let stack = load(&state, id).await?;
    if new.compose_yaml.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "The compose file is empty.".to_owned(),
        ));
    }
    state.store.stack_update_yaml(id, &new.compose_yaml).await?;
    // That it changed, not what it says: a compose file can hold secrets.
    crate::audit::record(&state, &principal, "edit compose file", &stack.slug, None).await;
    load(&state, id).await.map(Json)
}

/// Removes the registration only.
///
/// Containers are deliberately left alone: forgetting a stack and destroying
/// it are different intentions, and one must not silently perform the other.
/// Stop it first if that is what you meant. Its `.env` goes, since nothing
/// manages the secrets in it any more; the rest of its directory stays.
async fn remove(
    principal: Authorized<perm::StacksForget>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<axum::http::StatusCode, ApiError> {
    let stack = load(&state, id).await?;
    // Forgotten mid-deploy, the deploy would finish into a stack that no
    // longer exists and leave its files behind. Held until this is done, so
    // nothing can start in between either.
    let _slot = state.runner.claim(id).ok_or_else(|| {
        ApiError::Conflict("Something is running for this stack. Wait for it to finish.".to_owned())
    })?;
    state.store.stack_delete(id).await?;
    if let Err(e) = state.runner.forget_env(&stack.slug).await {
        tracing::warn!(error = %e, stack = %stack.slug, "could not remove a forgotten stack's .env");
    }
    crate::audit::record(&state, &principal, "forget stack", &stack.slug, None).await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn act(
    state: &AppState,
    principal: &Principal,
    id: i64,
    action: Action,
) -> Result<Json<Deployment>, ApiError> {
    let stack = load(state, id).await?;
    let started = state
        .runner
        .start(&stack, action, Trigger::Manual)
        .await
        .map_err(|e| match e {
            RunError::Busy => ApiError::Conflict(
                "Something is already running for this stack. Wait for it to finish.".to_owned(),
            ),
            RunError::Store(e) => ApiError::from(e),
            // Both carry the underlying tool's own message, which is the
            // useful part; a generic failure here would hide the reason.
            RunError::Compose(e) => ApiError::BadRequest(e.to_string()),
            RunError::Git(e) => ApiError::BadRequest(e.to_string()),
            e @ RunError::NoFiles(_) => ApiError::BadRequest(e.to_string()),
        })?;

    crate::audit::record(
        state,
        principal,
        &action.verb().to_lowercase(),
        &stack.slug,
        None,
    )
    .await;
    Ok(Json(started))
}

async fn deploy(
    principal: Authorized<perm::StacksDeploy>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Deployment>, ApiError> {
    act(&state, &principal, id, Action::Deploy).await
}

/// `compose down`: containers and networks go, named volumes stay.
/// The registration is kept, so the stack can be deployed again.
async fn down(
    principal: Authorized<perm::StacksTakeDown>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Deployment>, ApiError> {
    act(&state, &principal, id, Action::Remove).await
}

async fn stop(
    principal: Authorized<perm::StacksStop>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Deployment>, ApiError> {
    act(&state, &principal, id, Action::Stop).await
}

async fn restart(
    principal: Authorized<perm::StacksRestart>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Deployment>, ApiError> {
    act(&state, &principal, id, Action::Restart).await
}

async fn history(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<Deployment>>, ApiError> {
    load(&state, id).await?;
    Ok(Json(
        state.store.deployments_for_stack(id, HISTORY_LIMIT).await?,
    ))
}

async fn deployment(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<DeploymentDetail>, ApiError> {
    state
        .store
        .deployment_detail(id)
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound)
}
