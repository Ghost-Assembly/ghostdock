//! Git remotes, the credentials that reach them, and stack environments.
//!
//! Secrets are write-only here: they go in when created and no route ever
//! returns one, not even masked. A masked value still discloses its length.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use shared::deployment::RegisteredStack;
use shared::source::{
    Credential, EnvValue, EnvVar, NewCredential, NewGitStack, NewRepo, Repo, StackEnv, StackEnvKeys,
};

use crate::auth::{Authorized, perm};
use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/credentials",
            get(list_credentials).post(create_credential),
        )
        .route(
            "/credentials/{id}",
            axum::routing::delete(delete_credential),
        )
        .route("/repos", get(list_repos).post(create_repo))
        .route(
            "/repos/{id}",
            axum::routing::put(set_repo_credential).delete(delete_repo),
        )
        .route("/hosts/{host_id}/stacks/git", post(create_git_stack))
        .route("/stacks/{id}/env", get(list_env).put(set_env))
        .route(
            "/stacks/{id}/env/{key}",
            axum::routing::put(set_one_env).delete(delete_one_env),
        )
}

// ---- credentials ------------------------------------------------------

async fn list_credentials(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
) -> Result<Json<Vec<Credential>>, ApiError> {
    Ok(Json(state.store.credentials_list().await?))
}

async fn create_credential(
    principal: Authorized<perm::CredentialsManage>,
    State(state): State<AppState>,
    Json(new): Json<NewCredential>,
) -> Result<Json<Credential>, ApiError> {
    let name = new.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest(
            "Give the credential a name.".to_owned(),
        ));
    }
    if new.secret.is_empty() {
        return Err(ApiError::BadRequest(
            "A credential needs a token or password.".to_owned(),
        ));
    }

    // Most hosts want something here and ignore its value; an empty one makes
    // the Authorization header malformed rather than merely unusual.
    let username = if new.username.trim().is_empty() {
        "x-access-token"
    } else {
        new.username.trim()
    };

    let created = state
        .store
        .credential_create(name, username, &new.secret)
        .await
        .map_err(|e| match e {
            store::Error::SlugTaken => {
                ApiError::Conflict(format!("A credential named {name} already exists."))
            }
            other => ApiError::from(other),
        })?;

    // The name, never the secret. An audit trail that quotes what it
    // records would be the easiest place in the product to read one.
    crate::audit::record(&state, &principal, "add credential", &created.name, None).await;
    Ok(Json(created))
}

async fn delete_credential(
    principal: Authorized<perm::CredentialsManage>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<axum::http::StatusCode, ApiError> {
    state.store.credential_delete(id).await?;
    crate::audit::record(
        &state,
        &principal,
        "remove credential",
        &id.to_string(),
        None,
    )
    .await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

// ---- repositories -----------------------------------------------------

async fn list_repos(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
) -> Result<Json<Vec<Repo>>, ApiError> {
    Ok(Json(state.store.repos_list().await?))
}

async fn create_repo(
    principal: Authorized<perm::ReposManage>,
    State(state): State<AppState>,
    Json(new): Json<NewRepo>,
) -> Result<Json<Repo>, ApiError> {
    let url = new.url.trim();
    // Refused here, where the person typing it can fix it. git would read
    // some URLs as options or as commands to run.
    domain::source::check_repo_url(url).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    if let Some(id) = new.credential_id
        && state.store.credential_secret(id).await?.is_none()
    {
        return Err(ApiError::BadRequest(
            "That credential no longer exists.".to_owned(),
        ));
    }

    let created = state
        .store
        .repo_create(url, new.credential_id)
        .await
        .map_err(|e| match e {
            store::Error::SlugTaken => {
                ApiError::Conflict("That repository is already registered.".to_owned())
            }
            other => ApiError::from(other),
        })?;

    crate::audit::record(&state, &principal, "add repository", &created.url, None).await;
    Ok(Json(created))
}

async fn delete_repo(
    principal: Authorized<perm::ReposManage>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<axum::http::StatusCode, ApiError> {
    let repo = state
        .store
        .repo_by_id(id)
        .await?
        .ok_or(ApiError::NotFound)?;
    state.store.repo_delete(id).await.map_err(|e| match e {
        store::Error::InUse => ApiError::Conflict(
            "Stacks are still defined in this repository. Remove them first.".to_owned(),
        ),
        other => ApiError::from(other),
    })?;
    // Discovery's checkout of it is of no use to anyone now.
    if let Err(e) = state.runner.forget_discovery(id).await {
        tracing::warn!(error = %e, repo = id, "could not remove a repository's discovery checkout");
    }
    crate::audit::record(&state, &principal, "remove repository", &repo.url, None).await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

// ---- git-backed stacks ------------------------------------------------

/// Swaps the credential a repository is reached with, or removes it for a
/// public one. Tokens expire; replacing one must not mean re-registering
/// every stack that comes from the repository.
async fn set_repo_credential(
    principal: Authorized<perm::ReposManage>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(change): Json<shared::source::RepoCredential>,
) -> Result<Json<Repo>, ApiError> {
    if let Some(credential) = change.credential_id
        && state.store.credential_secret(credential).await?.is_none()
    {
        return Err(ApiError::BadRequest(
            "That credential no longer exists.".to_owned(),
        ));
    }
    let repo = state
        .store
        .repo_set_credential(id, change.credential_id)
        .await
        .map_err(|e| match e {
            store::Error::NotFound => ApiError::NotFound,
            other => ApiError::from(other),
        })?;
    let detail = repo.credential_name.clone().map_or_else(
        || "no credential".to_owned(),
        |name| format!("using {name}"),
    );
    crate::audit::record(
        &state,
        &principal,
        "change repository credential",
        &repo.url,
        Some(&detail),
    )
    .await;
    Ok(Json(repo))
}

async fn create_git_stack(
    principal: Authorized<perm::StacksCreate>,
    State(state): State<AppState>,
    Path(host_id): Path<i64>,
    Json(new): Json<NewGitStack>,
) -> Result<Json<RegisteredStack>, ApiError> {
    state
        .store
        .host_by_id(host_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let name = new.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("Give the stack a name.".to_owned()));
    }
    if state.store.repo_by_id(new.repo_id).await?.is_none() {
        return Err(ApiError::BadRequest(
            "That repository does not exist.".to_owned(),
        ));
    }

    let git_ref = new.git_ref.trim();
    domain::source::check_git_ref(git_ref).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    // Refuse a path that leaves the repository here, rather than at deploy
    // time: the person typing it is the one who can fix it.
    let compose_path = new.compose_path.trim();
    gitsync::resolve_in_repo(
        std::path::Path::new("/tmp/ghostdock-validate"),
        compose_path,
    )
    .map_err(|_| {
        ApiError::BadRequest("The compose file path must be inside the repository.".to_owned())
    })?;

    let slug = compose::slug::from_name(name).ok_or_else(|| {
        ApiError::BadRequest(
            "That name has no letters or digits to build an identifier from.".to_owned(),
        )
    })?;

    let created = state
        .store
        .stack_create_git(host_id, &slug, name, new.repo_id, git_ref, compose_path)
        .await
        .map_err(|e| match e {
            store::Error::SlugTaken => ApiError::Conflict(format!(
                "A stack named {slug} already exists. Compose projects must be unique."
            )),
            other => ApiError::from(other),
        })?;

    crate::audit::record(
        &state,
        &principal,
        "register stack",
        &created.slug,
        Some(compose_path),
    )
    .await;
    Ok(Json(created))
}

// ---- environment ------------------------------------------------------

/// Variable names only. Returning values would undo the point of sealing them.
async fn list_env(
    _principal: Authorized<perm::HostView>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<StackEnvKeys>, ApiError> {
    state
        .store
        .stack_by_id(id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(StackEnvKeys {
        keys: state.store.stack_env_keys(id).await?,
    }))
}

async fn set_env(
    principal: Authorized<perm::EnvWrite>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(env): Json<StackEnv>,
) -> Result<Json<StackEnvKeys>, ApiError> {
    let stack = state
        .store
        .stack_by_id(id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let vars: Vec<(String, String)> = env
        .vars
        .into_iter()
        .map(|EnvVar { key, value }| (key.trim().to_owned(), value))
        .filter(|(key, _)| !key.is_empty())
        .collect();

    // Validate by rendering what compose would actually read, so a value that
    // could define a second variable is refused by the person who typed it
    // rather than surfacing as a strange deploy later.
    compose::env::render(&vars).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    state.store.stack_env_set(id, &vars).await?;
    // The names, never the values.
    let names: Vec<&str> = vars.iter().map(|(key, _)| key.as_str()).collect();
    crate::audit::record(
        &state,
        &principal,
        "replace variables",
        &stack.slug,
        Some(&names.join(", ")),
    )
    .await;
    Ok(Json(StackEnvKeys {
        keys: state.store.stack_env_keys(id).await?,
    }))
}

/// Sets one variable, leaving the rest alone.
///
/// The per-key route exists because values are write-only: with only a
/// wholesale replace, changing one variable would mean retyping every secret
/// the stack has.
async fn set_one_env(
    principal: Authorized<perm::EnvWrite>,
    State(state): State<AppState>,
    Path((id, key)): Path<(i64, String)>,
    Json(body): Json<EnvValue>,
) -> Result<Json<StackEnvKeys>, ApiError> {
    state
        .store
        .stack_by_id(id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let key = key.trim().to_owned();
    // Validated against what compose would actually read, so the person
    // typing finds out rather than a deploy failing strangely later.
    compose::env::render(&[(key.clone(), body.value.clone())])
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    state.store.stack_env_set_one(id, &key, &body.value).await?;
    // The variable's name, never its value.
    crate::audit::record(&state, &principal, "set variable", &key, None).await;
    Ok(Json(StackEnvKeys {
        keys: state.store.stack_env_keys(id).await?,
    }))
}

async fn delete_one_env(
    principal: Authorized<perm::EnvWrite>,
    State(state): State<AppState>,
    Path((id, key)): Path<(i64, String)>,
) -> Result<Json<StackEnvKeys>, ApiError> {
    state
        .store
        .stack_by_id(id)
        .await?
        .ok_or(ApiError::NotFound)?;
    state.store.stack_env_delete_one(id, &key).await?;
    crate::audit::record(&state, &principal, "remove variable", &key, None).await;
    Ok(Json(StackEnvKeys {
        keys: state.store.stack_env_keys(id).await?,
    }))
}
