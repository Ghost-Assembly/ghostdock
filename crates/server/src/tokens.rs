//! Issuing and revoking API tokens.
//!
//! Every handler here takes a bare [`Principal`], so only a person signed in
//! through the browser reaches them. A token that could issue tokens could
//! grant itself anything.

use axum::Json;
use axum::extract::{Path, State};
use shared::reference::Access;
use shared::token::{ApiToken, CreatedApiToken, NewApiToken};
use store::tokens::TokenRow;

use crate::auth::Principal;
use crate::error::ApiError;
use crate::reference::Routes;
use crate::revocation::Revocation;
use crate::state::AppState;

const MAX_NAME_LEN: usize = 64;
const MAX_EXPIRY_DAYS: u32 = 3650;

pub fn routes() -> Routes {
    Routes::new("API tokens")
        .get(
            "/tokens",
            Access::Session,
            "Your tokens and what each was granted; never their secrets",
            list,
        )
        .post(
            "/tokens",
            Access::Session,
            "Issues a token with the permissions given; its secret is returned this once",
            create,
        )
        .delete(
            "/tokens/{id}",
            Access::Session,
            "Revokes one of your tokens and ends what it holds open",
            revoke,
        )
}

fn to_wire(row: TokenRow) -> ApiToken {
    ApiToken {
        id: row.id,
        name: row.name,
        prefix: row.prefix,
        permissions: row.permissions,
        created_at: store::timestamp(row.created_at),
        last_used_at: row.last_used_at.map(store::timestamp),
        expires_at: row.expires_at.map(store::timestamp),
    }
}

async fn list(
    principal: Principal,
    State(state): State<AppState>,
) -> Result<Json<Vec<ApiToken>>, ApiError> {
    let rows = state.store.tokens_for_user(principal.user.id).await?;
    Ok(Json(rows.into_iter().map(to_wire).collect()))
}

async fn create(
    principal: Principal,
    State(state): State<AppState>,
    Json(new): Json<NewApiToken>,
) -> Result<Json<CreatedApiToken>, ApiError> {
    let name = new.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
        return Err(ApiError::BadRequest(format!(
            "a token needs a name of 1 to {MAX_NAME_LEN} characters"
        )));
    }
    if new.permissions.is_empty() {
        return Err(ApiError::BadRequest(
            "a token needs at least one permission".to_owned(),
        ));
    }
    let expires_at = match new.expires_in_days {
        None => None,
        Some(days @ 1..=MAX_EXPIRY_DAYS) => {
            Some(chrono::Utc::now().timestamp() + i64::from(days) * 86_400)
        }
        Some(_) => {
            return Err(ApiError::BadRequest(format!(
                "expiry must be between 1 and {MAX_EXPIRY_DAYS} days"
            )));
        }
    };

    let (row, secret) = state
        .store
        .token_create(principal.user.id, name, &new.permissions, expires_at)
        .await
        .map_err(|e| match e {
            store::Error::NameTaken => {
                ApiError::Conflict("you already have a token with that name".to_owned())
            }
            other => ApiError::from(other),
        })?;

    let granted = row
        .permissions
        .iter()
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    crate::audit::record(
        &state,
        &principal,
        "create token",
        &row.name,
        Some(&granted),
    )
    .await;
    Ok(Json(CreatedApiToken {
        token: to_wire(row),
        secret,
    }))
}

async fn revoke(
    principal: Principal,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<axum::http::StatusCode, ApiError> {
    let name = state
        .store
        .tokens_for_user(principal.user.id)
        .await?
        .into_iter()
        .find(|t| t.id == id)
        .map(|t| t.name)
        .ok_or(ApiError::NotFound)?;
    state
        .store
        .token_delete(principal.user.id, id)
        .await
        .map_err(|e| match e {
            store::Error::NotFound => ApiError::NotFound,
            other => ApiError::from(other),
        })?;
    state.revocations.revoke(Revocation::Token(id));
    crate::audit::record(&state, &principal, "revoke token", &name, None).await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
