//! Finding compose files in a repository and registering them as stacks.
//!
//! An importer on top of ordinary registration: every stack it creates is a
//! plain Git-backed stack, exactly as if it had been added by hand.

use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use domain::discovery::{DEFAULT_PATTERN, stack_name};
use domain::glob::Pattern;
use shared::deployment::RegisteredStack;
use shared::source::{
    DiscoverRequest, Discovered, DiscoveredStatus, Discovery, ImportRequest, ImportResult, Repo,
};
use store::hosts::LOCAL_HOST_ID;

use crate::auth::{Authorized, perm};
use crate::error::ApiError;
use crate::runner::RunError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{id}/discover", post(discover))
        .route("/repos/{id}/import", post(import))
}

async fn discover(
    _principal: Authorized<perm::StacksCreate>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(request): Json<DiscoverRequest>,
) -> Result<Json<Discovery>, ApiError> {
    let repo = state
        .store
        .repo_by_id(id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(
        look(&state, &repo, &request.git_ref, request.pattern.as_deref()).await?,
    ))
}

/// Registers the chosen files. Discovery runs again here rather than
/// trusting paths from the client: only a file discovery offers as new is
/// registered, whatever was sent.
async fn import(
    principal: Authorized<perm::StacksCreate>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(request): Json<ImportRequest>,
) -> Result<Json<ImportResult>, ApiError> {
    let principal = principal.clone();
    // Detached: an import abandoned halfway would leave some stacks
    // registered and some not.
    crate::detach::finish(async move { import_all(state, principal, id, request).await })
        .await
        .map(Json)
}

async fn import_all(
    state: AppState,
    principal: crate::auth::Principal,
    id: i64,
    request: ImportRequest,
) -> Result<ImportResult, ApiError> {
    let repo = state
        .store
        .repo_by_id(id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let discovery = look(&state, &repo, &request.git_ref, request.pattern.as_deref()).await?;
    let git_ref = request.git_ref.trim();

    let mut result = ImportResult {
        created: Vec::new(),
        skipped: Vec::new(),
    };
    for path in request.paths {
        let Some(found) = discovery.found.iter().find(|f| f.path == path) else {
            result
                .skipped
                .push((path, "not a compose file this search found".to_owned()));
            continue;
        };
        let reason = match &found.status {
            DiscoveredStatus::New => None,
            DiscoveredStatus::Registered { stack_name, .. } => {
                Some(format!("already registered as {stack_name}"))
            }
            DiscoveredStatus::NameTaken { stack_name } => {
                Some(format!("the name is taken by {stack_name}"))
            }
        };
        if let Some(reason) = reason {
            result.skipped.push((path, reason));
            continue;
        }
        let Some(slug) = compose::slug::from_name(&found.name) else {
            result
                .skipped
                .push((path, "no usable name could be made from it".to_owned()));
            continue;
        };
        match state
            .store
            .stack_create_git(LOCAL_HOST_ID, &slug, &found.name, repo.id, git_ref, &path)
            .await
        {
            Ok(stack) => result.created.push(stack),
            Err(store::Error::SlugTaken) => {
                result
                    .skipped
                    .push((path, "the name was just taken".to_owned()));
            }
            Err(e) => return Err(e.into()),
        }
    }

    if !result.created.is_empty() {
        let names: Vec<_> = result.created.iter().map(|s| s.slug.as_str()).collect();
        crate::audit::record(
            &state,
            &principal,
            "import stacks",
            &repo.url,
            Some(&names.join(", ")),
        )
        .await;
    }
    Ok(result)
}

async fn look(
    state: &AppState,
    repo: &Repo,
    git_ref: &str,
    pattern: Option<&str>,
) -> Result<Discovery, ApiError> {
    let git_ref = git_ref.trim();
    domain::source::check_git_ref(git_ref).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let source = pattern
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or(DEFAULT_PATTERN);
    let compiled =
        Pattern::parse(source).map_err(|e| ApiError::BadRequest(format!("The pattern: {e}.")))?;

    let (commit, paths) = state
        .runner
        .discover(repo.id, &repo.url, git_ref, &compiled)
        .await
        .map_err(|e| match e {
            RunError::Git(e) => ApiError::BadRequest(e.to_string()),
            RunError::Store(e) => ApiError::from(e),
            other => ApiError::Internal(anyhow::Error::new(other)),
        })?;

    let registered = state.store.stacks_list(LOCAL_HOST_ID).await?;
    let found = paths
        .into_iter()
        .map(|path| {
            let name = stack_name(&path).unwrap_or_else(|| repo_name(&repo.url));
            let status = status_of(&path, &name, repo.id, &registered);
            Discovered { path, name, status }
        })
        .collect();

    Ok(Discovery {
        commit,
        pattern: source.to_owned(),
        found,
    })
}

fn status_of(
    path: &str,
    name: &str,
    repo_id: i64,
    registered: &[RegisteredStack],
) -> DiscoveredStatus {
    if let Some(stack) = registered.iter().find(|s| {
        s.git
            .as_ref()
            .is_some_and(|g| g.repo_id == repo_id && g.compose_path == path)
    }) {
        return DiscoveredStatus::Registered {
            stack_id: stack.id,
            stack_name: stack.name.clone(),
        };
    }
    let slug = compose::slug::from_name(name);
    if let Some(stack) = registered.iter().find(|s| Some(&s.slug) == slug.as_ref()) {
        return DiscoveredStatus::NameTaken {
            stack_name: stack.name.clone(),
        };
    }
    DiscoveredStatus::New
}

/// A repository's name from its URL: the last path segment, without `.git`.
fn repo_name(url: &str) -> String {
    let last = url
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(url);
    last.strip_suffix(".git").unwrap_or(last).to_owned()
}
