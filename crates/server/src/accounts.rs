//! Other people's accounts: listing, adding, removing.
//!
//! Every account can do everything. There are no roles in v1, so there is
//! nothing here to decide who may manage whom; the rules are only the ones
//! that keep the instance from ending up with no one able to sign in.

use axum::Json;
use axum::extract::{Path, State};
use shared::auth::{Account, Credentials};
use shared::reference::Access;
use store::users::UserRow;

use crate::auth::Principal;
use crate::error::ApiError;
use crate::reference::Routes;
use crate::revocation::Revocation;
use crate::state::AppState;

pub fn routes() -> Routes {
    Routes::new("Accounts")
        .get("/users", Access::Session, "Every account", list)
        .post(
            "/users",
            Access::Session,
            "Adds an account with a username and password",
            create,
        )
        .delete(
            "/users/{id}",
            Access::Session,
            "Removes another account and ends its sessions and tokens",
            remove,
        )
}

fn account(row: &UserRow, caller: i64) -> Account {
    Account {
        id: row.id,
        username: row.username.clone(),
        created_at: store::timestamp(row.created_at),
        you: row.id == caller,
    }
}

async fn list(
    principal: Principal,
    State(state): State<AppState>,
) -> Result<Json<Vec<Account>>, ApiError> {
    let rows = state.store.users_list().await?;
    Ok(Json(
        rows.iter().map(|r| account(r, principal.user.id)).collect(),
    ))
}

async fn create(
    principal: Principal,
    State(state): State<AppState>,
    Json(credentials): Json<Credentials>,
) -> Result<Json<Account>, ApiError> {
    let username = credentials.username.trim();
    if username.is_empty() {
        return Err(ApiError::BadRequest(
            "username must not be empty".to_owned(),
        ));
    }
    let hash = crate::auth::hash_blocking(credentials.password).await?;
    let row = state
        .store
        .user_create(username, &hash)
        .await
        .map_err(|e| match e {
            store::Error::UsernameTaken => ApiError::Conflict("that username is taken".to_owned()),
            other => ApiError::from(other),
        })?;
    crate::audit::record(&state, &principal, "add account", &row.username, None).await;
    Ok(Json(account(&row, principal.user.id)))
}

async fn remove(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<axum::http::StatusCode, ApiError> {
    // Refusing your own account is what guarantees one always remains: the
    // caller is an account, and it is not the one being removed.
    if id == principal.user.id {
        return Err(ApiError::Conflict(
            "you cannot remove your own account; sign in as someone else to do that".to_owned(),
        ));
    }
    let target = state
        .store
        .user_by_id(id)
        .await?
        .ok_or(ApiError::NotFound)?;
    state.store.user_delete(id).await.map_err(|e| match e {
        store::Error::NotFound => ApiError::NotFound,
        store::Error::LastAccount => {
            ApiError::Conflict("the last account cannot be removed".to_owned())
        }
        other => ApiError::from(other),
    })?;
    state.revocations.revoke(Revocation::Account(id));
    crate::audit::record(&state, &principal, "remove account", &target.username, None).await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
