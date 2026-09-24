//! Authentication: first-run bootstrap, login, logout, and the extractor
//! every protected handler depends on.

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use domain::auth::{hash_password, verify_password};
use shared::auth::{AuthStatus, Credentials, PasswordChange, User};
use shared::token::Permission;
use std::marker::PhantomData;
use std::ops::Deref;
use tower_sessions::Session;

use crate::error::ApiError;
use crate::state::AppState;

/// Session key holding the authenticated user's id.
const SESSION_USER_ID: &str = "user_id";
/// Session key holding the account's session epoch when the session began.
/// Absent on sessions from before epochs existed, which count as zero.
const SESSION_EPOCH: &str = "epoch";

/// How the caller proved who they are.
#[derive(Debug, Clone)]
pub enum Via {
    /// A person signed in through the browser. Can do everything.
    Session,
    /// An API token, which can do only what it was granted.
    Token {
        id: i64,
        name: String,
        permissions: Vec<Permission>,
    },
}

/// The authenticated caller.
///
/// Extracting a bare `Principal` admits **sessions only**: a token presented
/// to such a handler is refused. Handlers a token may reach take
/// [`Authorized`] instead, naming the permission they need. Forgetting to
/// choose therefore fails closed, never open.
#[derive(Debug, Clone)]
pub struct Principal {
    pub user: User,
    pub via: Via,
}

impl Principal {
    #[must_use]
    pub fn can(&self, permission: Permission) -> bool {
        match &self.via {
            Via::Session => true,
            Via::Token { permissions, .. } => permissions.contains(&permission),
        }
    }

    /// Who to name in the audit trail. A token is named alongside its
    /// account, so "who did this" has an answer that can be revoked.
    #[must_use]
    pub fn display_name(&self) -> String {
        match &self.via {
            Via::Session => self.user.username.clone(),
            Via::Token { name, .. } => format!("{} (token {name})", self.user.username),
        }
    }

    /// Resolves the caller from a bearer token if one was presented, else
    /// from the session cookie. A presented token is never silently ignored
    /// in favour of a cookie: an invalid one is a failed request.
    async fn resolve(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        if let Some(header) = parts.headers.get(axum::http::header::AUTHORIZATION) {
            let secret = header
                .to_str()
                .ok()
                .and_then(|v| {
                    v.strip_prefix("Bearer ")
                        .or_else(|| v.strip_prefix("bearer "))
                })
                .map(str::trim)
                .ok_or(ApiError::NotAuthenticated)?;
            let (token, user) = state
                .store
                .token_authenticate(secret)
                .await?
                .ok_or(ApiError::NotAuthenticated)?;
            return Ok(Self {
                user: user.to_public(),
                via: Via::Token {
                    id: token.id,
                    name: token.name,
                    permissions: token.permissions,
                },
            });
        }

        let session = Session::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::NotAuthenticated)?;

        let user_id: i64 = session
            .get(SESSION_USER_ID)
            .await?
            .ok_or(ApiError::NotAuthenticated)?;

        // Re-read the account on every request rather than trusting the
        // session payload, so deleting a user immediately invalidates their
        // sessions instead of leaving them valid until expiry.
        let row = state
            .store
            .user_by_id(user_id)
            .await?
            .ok_or(ApiError::NotAuthenticated)?;

        // A password change moves the epoch on, voiding every session begun
        // under the old password.
        let epoch: i64 = session.get(SESSION_EPOCH).await?.unwrap_or(0);
        if epoch != row.session_epoch {
            return Err(ApiError::NotAuthenticated);
        }

        Ok(Self {
            user: row.to_public(),
            via: Via::Session,
        })
    }
}

impl FromRequestParts<AppState> for Principal {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let principal = Self::resolve(parts, state).await?;
        match principal.via {
            Via::Session => Ok(principal),
            Via::Token { .. } => Err(ApiError::SessionOnly),
        }
    }
}

/// A caller allowed to do `P`: any signed-in person, or a token granted it.
///
/// Dereferences to the [`Principal`], so it can be passed wherever one is
/// expected (the audit trail, chiefly).
#[derive(Debug)]
pub struct Authorized<P> {
    principal: Principal,
    _permission: PhantomData<P>,
}

impl<P> Deref for Authorized<P> {
    type Target = Principal;
    fn deref(&self) -> &Principal {
        &self.principal
    }
}

impl<P: perm::Required + Send + Sync> FromRequestParts<AppState> for Authorized<P> {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let principal = Principal::resolve(parts, state).await?;
        if !principal.can(P::PERMISSION) {
            return Err(ApiError::MissingPermission(P::PERMISSION));
        }
        Ok(Self {
            principal,
            _permission: PhantomData,
        })
    }
}

/// One marker type per [`Permission`], for naming in handler signatures:
/// `principal: Authorized<perm::StacksDeploy>`.
pub mod perm {
    use shared::token::Permission;

    pub trait Required {
        const PERMISSION: Permission;
    }

    macro_rules! markers {
        ($($name:ident),* $(,)?) => {$(
            #[derive(Debug)]
            pub struct $name;
            impl Required for $name {
                const PERMISSION: Permission = Permission::$name;
            }
        )*};
    }

    markers!(
        HostView,
        ComposeRead,
        LogsView,
        ActivityView,
        StacksDeploy,
        StacksRestart,
        StacksStop,
        StacksTakeDown,
        UpdatesCheck,
        UpdatesAutoApply,
        StacksCreate,
        StacksEdit,
        StacksForget,
        EnvWrite,
        ReposManage,
        CredentialsManage,
        CleanupRun,
        ShellOpen,
    );
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/status", get(status))
        .route("/auth/bootstrap", post(bootstrap))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/password", put(change_password))
}

/// Unauthenticated: tells the client whether to show setup or login.
async fn status(
    State(state): State<AppState>,
    session: Session,
) -> Result<Json<AuthStatus>, ApiError> {
    let bootstrapped = state.store.user_count().await? > 0;

    let user = match session.get::<i64>(SESSION_USER_ID).await? {
        Some(id) => {
            let epoch: i64 = session.get(SESSION_EPOCH).await?.unwrap_or(0);
            state
                .store
                .user_by_id(id)
                .await?
                .filter(|u| u.session_epoch == epoch)
                .map(|u| u.to_public())
        }
        None => None,
    };

    Ok(Json(AuthStatus { bootstrapped, user }))
}

/// Creates the first administrator.
///
/// Permitted only while no account exists. This is the only way an account
/// comes into being, so there is never a default credential to forget to
/// change — which matters for something holding the Docker socket.
async fn bootstrap(
    State(state): State<AppState>,
    session: Session,
    Json(credentials): Json<Credentials>,
) -> Result<Json<User>, ApiError> {
    if state.store.user_count().await? > 0 {
        return Err(ApiError::Conflict(
            "this instance already has an administrator".to_owned(),
        ));
    }

    let username = credentials.username.trim();
    if username.is_empty() {
        return Err(ApiError::BadRequest(
            "username must not be empty".to_owned(),
        ));
    }

    let hash =
        hash_password(&credentials.password).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let row = state
        .store
        .user_create(username, &hash)
        .await
        .map_err(|e| match e {
            store::Error::UsernameTaken => ApiError::Conflict("that username is taken".to_owned()),
            other => ApiError::from(other),
        })?;

    start_session(&session, &row).await?;
    crate::audit::record_anonymous(&state, username, "bootstrap", "administrator account").await;
    Ok(Json(row.to_public()))
}

async fn login(
    State(state): State<AppState>,
    session: Session,
    Json(credentials): Json<Credentials>,
) -> Result<Json<User>, ApiError> {
    let row = state
        .store
        .user_by_username(credentials.username.trim())
        .await?;

    // Verify even when the user does not exist, against a hash that cannot
    // match, so a missing account and a wrong password take the same time.
    // Otherwise response latency enumerates valid usernames.
    let stored = row
        .as_ref()
        .map_or(DUMMY_HASH, |r| r.password_hash.as_str());
    let password_ok = verify_password(&credentials.password, stored);

    match row {
        Some(row) if password_ok => {
            start_session(&session, &row).await?;
            crate::audit::record_anonymous(&state, &row.username, "sign in", "ghostdock").await;
            Ok(Json(row.to_public()))
        }
        _ => {
            // Failed attempts are recorded too: a run of them is the thing
            // an audit trail exists to make visible.
            crate::audit::record_anonymous(
                &state,
                credentials.username.trim(),
                "failed sign in",
                "ghostdock",
            )
            .await;
            Err(ApiError::InvalidCredentials)
        }
    }
}

async fn logout(session: Session) -> Result<axum::http::StatusCode, ApiError> {
    // `flush` deletes the record server-side, so the cookie is worthless even
    // if it was captured.
    session.flush().await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Replaces the caller's own password.
///
/// Asks for the current one even though the caller is signed in: a session
/// left open on a borrowed phone should not be enough to lock its owner out.
async fn change_password(
    principal: Principal,
    State(state): State<AppState>,
    session: Session,
    Json(change): Json<PasswordChange>,
) -> Result<axum::http::StatusCode, ApiError> {
    let row = state
        .store
        .user_by_id(principal.user.id)
        .await?
        .ok_or(ApiError::NotAuthenticated)?;
    if !verify_password(&change.current, &row.password_hash) {
        return Err(ApiError::WrongPassword);
    }
    let hash = hash_password(&change.new).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let epoch = state.store.user_set_password(row.id, &hash).await?;

    // Every other session is now void. This one carries on, under a new id.
    // Its own open streams end too; the browser reconnects them at once with
    // the cookie it now holds.
    session.cycle_id().await?;
    session.insert(SESSION_EPOCH, epoch).await?;
    state
        .revocations
        .revoke(crate::revocation::Revocation::Sessions(row.id));
    crate::audit::record(&state, &principal, "change password", &row.username, None).await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn start_session(session: &Session, user: &store::users::UserRow) -> Result<(), ApiError> {
    // Issue a fresh session id at every privilege change, so a cookie planted
    // before login cannot be used to ride the authenticated session.
    session.cycle_id().await?;
    session.insert(SESSION_USER_ID, user.id).await?;
    session.insert(SESSION_EPOCH, user.session_epoch).await?;
    Ok(())
}

/// A real argon2id hash of a value no one can supply, used to keep the
/// timing of a failed login independent of whether the account exists.
const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHR2YWx1ZQ$\
    7NHQvGSAZ7WYpJHTvDLLFqgLU7wPBGnRVcVRQMDMK8Q";
