//! HTTP client for the GhostDock API.
//!
//! The server speaks plain JSON over REST, so this is a thin, replaceable
//! layer — which is the point: no domain logic lives here, only transport.

use std::cell::RefCell;
use std::rc::Rc;

use gloo_net::http::{Request, RequestBuilder, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use shared::audit::AuditEntry;
use shared::auth::{Account, AuthStatus, Credentials, PasswordChange, User};
use shared::cleanup::{CleanupPreview, CleanupRequest, CleanupResult, CleanupScope};
use shared::deployment::{Deployment, DeploymentDetail, NewStack, RegisteredStack, StackCompose};
use shared::host::{Host, HostInfo};
use shared::logs::Logs;
use shared::metrics::{ContainerFigures, Now, Range, Recommendation, Series, Target};
use shared::source::{
    Credential, DiscoverRequest, Discovery, ImportRequest, ImportResult, NewCredential,
    NewGitStack, NewRepo, Repo, StackEnv, StackEnvKeys,
};
use shared::stack::Stack;
use shared::token::{ApiToken, CreatedApiToken, NewApiToken};
use shared::update::{AutoApply, StackUpdate, UpdateStatus};

const BASE: &str = "/api/v1";

/// How long a read may take before its screen stops waiting and says so.
const READ_TIMEOUT_MS: u32 = 15_000;
/// How long an action may take. Longer, because some legitimately take a
/// while; and the server carries on regardless, so only the wait ends.
const ACT_TIMEOUT_MS: u32 = 60_000;

/// Every request is built here, with a deadline. Without one, a server that
/// stops answering leaves a screen saying "Loading" for good. Clippy refuses
/// gloo's builders anywhere else in this crate.
#[allow(clippy::disallowed_methods)]
fn request(method: &str, url: &str) -> RequestBuilder {
    let (builder, deadline) = match method {
        "GET" => (Request::get(url), READ_TIMEOUT_MS),
        "PUT" => (Request::put(url), ACT_TIMEOUT_MS),
        "DELETE" => (Request::delete(url), ACT_TIMEOUT_MS),
        _ => (Request::post(url), ACT_TIMEOUT_MS),
    };
    let signal = web_sys::AbortSignal::timeout_with_u32(deadline);
    builder.abort_signal(Some(&signal))
}

/// A failed request, carrying something worth showing a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    /// Present when the server answered; absent when the network did not.
    pub status: Option<u16>,
}

impl Error {
    fn network(detail: &str) -> Self {
        let message = if detail.contains("TimeoutError") || detail.contains("timed out") {
            "GhostDock did not answer in time. Anything it was asked to do carries on; \
             try again in a moment."
                .to_owned()
        } else {
            format!("Could not reach ghostdock: {detail}")
        };
        Self {
            message,
            status: None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

async fn read<T: DeserializeOwned>(response: Response) -> Result<T> {
    let status = response.status();
    if (200..300).contains(&status) {
        return response.json::<T>().await.map_err(|e| Error {
            message: format!("GhostDock sent a response we could not read: {e}"),
            status: Some(status),
        });
    }

    Err(refusal(response).await)
}

thread_local! {
    static ON_UNAUTHENTICATED: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Runs `f` whenever the server answers 401: the session has ended, by
/// expiry, a password change or signing out elsewhere. Handled here, once,
/// so every screen falls back to sign-in rather than showing an
/// authentication error where its content should be.
pub fn on_unauthenticated(f: impl Fn() + 'static) {
    ON_UNAUTHENTICATED.with(|slot| *slot.borrow_mut() = Some(Rc::new(f)));
}

/// The error for a response that was not a success. Surfaces the server's
/// own message, which was written to be shown.
async fn refusal(response: Response) -> Error {
    let status = response.status();
    if status == 401 {
        // Taken out first: the hook may make requests of its own.
        if let Some(hook) = ON_UNAUTHENTICATED.with(|slot| slot.borrow().clone()) {
            hook();
        }
    }
    let message = response.json::<shared::ApiError>().await.map_or_else(
        |_| format!("Request failed with status {status}"),
        |e| e.message,
    );
    Error {
        message,
        status: Some(status),
    }
}

async fn get<T: DeserializeOwned>(path: &str) -> Result<T> {
    let response = request("GET", &format!("{BASE}{path}"))
        .send()
        .await
        .map_err(|e| Error::network(&e.to_string()))?;
    read(response).await
}

async fn post<B: Serialize, T: DeserializeOwned>(path: &str, body: &B) -> Result<T> {
    let response = request("POST", &format!("{BASE}{path}"))
        .json(body)
        .map_err(|e| Error::network(&e.to_string()))?
        .send()
        .await
        .map_err(|e| Error::network(&e.to_string()))?;
    read(response).await
}

async fn post_empty<T: DeserializeOwned>(path: &str) -> Result<T> {
    let response = request("POST", &format!("{BASE}{path}"))
        .send()
        .await
        .map_err(|e| Error::network(&e.to_string()))?;
    read(response).await
}

pub async fn auth_status() -> Result<AuthStatus> {
    get("/auth/status").await
}

pub async fn bootstrap(credentials: &Credentials) -> Result<User> {
    post("/auth/bootstrap", credentials).await
}

pub async fn login(credentials: &Credentials) -> Result<User> {
    post("/auth/login", credentials).await
}

pub async fn accounts() -> Result<Vec<Account>> {
    get("/users").await
}

pub async fn add_account(credentials: &Credentials) -> Result<Account> {
    post("/users", credentials).await
}

pub async fn remove_account(id: i64) -> Result<()> {
    delete(&format!("/users/{id}")).await
}

pub async fn change_password(change: &PasswordChange) -> Result<()> {
    let response = request("PUT", &format!("{BASE}/auth/password"))
        .json(change)
        .map_err(|e| Error::network(&e.to_string()))?
        .send()
        .await
        .map_err(|e| Error::network(&e.to_string()))?;
    if (200..300).contains(&response.status()) {
        return Ok(());
    }
    read::<()>(response).await
}

pub async fn tokens() -> Result<Vec<ApiToken>> {
    get("/tokens").await
}

pub async fn create_token(new: &NewApiToken) -> Result<CreatedApiToken> {
    post("/tokens", new).await
}

pub async fn revoke_token(id: i64) -> Result<()> {
    delete(&format!("/tokens/{id}")).await
}

pub async fn logout() -> Result<()> {
    let response = request("POST", &format!("{BASE}/auth/logout"))
        .send()
        .await
        .map_err(|e| Error::network(&e.to_string()))?;

    let status = response.status();
    // A 401 means there was no session left to end: signed out either way.
    if (200..300).contains(&status) || status == 401 {
        Ok(())
    } else {
        Err(Error {
            message: "Could not sign out".to_owned(),
            status: Some(status),
        })
    }
}

pub async fn hosts() -> Result<Vec<Host>> {
    get("/hosts").await
}

pub async fn host_info(host_id: i64) -> Result<HostInfo> {
    get(&format!("/hosts/{host_id}")).await
}

pub async fn stacks(host_id: i64) -> Result<Vec<Stack>> {
    get(&format!("/hosts/{host_id}/stacks")).await
}

// ---- stacks -----------------------------------------------------------

pub async fn create_stack(host_id: i64, new: &NewStack) -> Result<RegisteredStack> {
    post(&format!("/hosts/{host_id}/stacks"), new).await
}

pub async fn stack(id: i64) -> Result<RegisteredStack> {
    get(&format!("/stacks/{id}")).await
}

/// The stack together with its stored compose file, for the edit form.
pub async fn stack_with_yaml(id: i64) -> Result<(RegisteredStack, String)> {
    let stack = stack(id).await?;
    let compose: StackCompose = get(&format!("/stacks/{id}/compose")).await?;
    Ok((stack, compose.compose_yaml))
}

pub async fn update_stack(id: i64, new: &NewStack) -> Result<RegisteredStack> {
    let response = request("PUT", &format!("{BASE}/stacks/{id}"))
        .json(new)
        .map_err(|e| Error::network(&e.to_string()))?
        .send()
        .await
        .map_err(|e| Error::network(&e.to_string()))?;
    read(response).await
}

pub async fn delete_stack(id: i64) -> Result<()> {
    delete(&format!("/stacks/{id}")).await
}

/// A DELETE that expects no body back.
async fn delete(path: &str) -> Result<()> {
    let response = request("DELETE", &format!("{BASE}{path}"))
        .send()
        .await
        .map_err(|e| Error::network(&e.to_string()))?;

    let status = response.status();
    if (200..300).contains(&status) {
        return Ok(());
    }

    // The server explains conflicts ("stacks are still defined in this
    // repository"), and that explanation is the whole value of the response.
    Err(refusal(response).await)
}

/// Starts an operation. Returns as soon as it is recorded, not when it ends;
/// progress arrives on the event stream.
pub async fn act(id: i64, action: &str) -> Result<Deployment> {
    post_empty(&format!("/stacks/{id}/{action}")).await
}

pub async fn deployments(stack_id: i64) -> Result<Vec<Deployment>> {
    get(&format!("/stacks/{stack_id}/deployments")).await
}

pub async fn deployment(id: i64) -> Result<DeploymentDetail> {
    get(&format!("/deployments/{id}")).await
}

// ---- sources ----------------------------------------------------------

pub async fn credentials() -> Result<Vec<Credential>> {
    get("/credentials").await
}

pub async fn set_repo_credential(repo_id: i64, credential_id: Option<i64>) -> Result<Repo> {
    let response = request("PUT", &format!("{BASE}/repos/{repo_id}"))
        .json(&shared::source::RepoCredential { credential_id })
        .map_err(|e| Error::network(&e.to_string()))?
        .send()
        .await
        .map_err(|e| Error::network(&e.to_string()))?;
    read(response).await
}

pub async fn discover(repo_id: i64, request: &DiscoverRequest) -> Result<Discovery> {
    post(&format!("/repos/{repo_id}/discover"), request).await
}

pub async fn import(repo_id: i64, request: &ImportRequest) -> Result<ImportResult> {
    post(&format!("/repos/{repo_id}/import"), request).await
}

pub async fn create_credential(new: &NewCredential) -> Result<Credential> {
    post("/credentials", new).await
}

pub async fn delete_credential(id: i64) -> Result<()> {
    delete(&format!("/credentials/{id}")).await
}

pub async fn repos() -> Result<Vec<Repo>> {
    get("/repos").await
}

pub async fn create_repo(new: &NewRepo) -> Result<Repo> {
    post("/repos", new).await
}

pub async fn delete_repo(id: i64) -> Result<()> {
    delete(&format!("/repos/{id}")).await
}

pub async fn create_git_stack(host_id: i64, new: &NewGitStack) -> Result<RegisteredStack> {
    post(&format!("/hosts/{host_id}/stacks/git"), new).await
}

/// Variable names only; values never leave the server.
pub async fn stack_env_keys(stack_id: i64) -> Result<StackEnvKeys> {
    get(&format!("/stacks/{stack_id}/env")).await
}

/// One variable's address. The name is encoded, so whatever was typed stays
/// one path segment; screens also refuse names the server would.
fn env_url(stack_id: i64, key: &str) -> String {
    format!("{BASE}/stacks/{stack_id}/env/{}", component(key))
}

/// Sets one variable, leaving the others untouched.
pub async fn set_stack_env_one(
    stack_id: i64,
    key: &str,
    body: &shared::source::EnvValue,
) -> Result<StackEnvKeys> {
    let response = request("PUT", &env_url(stack_id, key))
        .json(body)
        .map_err(|e| Error::network(&e.to_string()))?
        .send()
        .await
        .map_err(|e| Error::network(&e.to_string()))?;
    read(response).await
}

pub async fn delete_stack_env_one(stack_id: i64, key: &str) -> Result<StackEnvKeys> {
    let response = request("DELETE", &env_url(stack_id, key))
        .send()
        .await
        .map_err(|e| Error::network(&e.to_string()))?;
    read(response).await
}

#[allow(dead_code)]
pub async fn set_stack_env(stack_id: i64, env: &StackEnv) -> Result<StackEnvKeys> {
    let response = request("PUT", &format!("{BASE}/stacks/{stack_id}/env"))
        .json(env)
        .map_err(|e| Error::network(&e.to_string()))?
        .send()
        .await
        .map_err(|e| Error::network(&e.to_string()))?;
    read(response).await
}

// ---- updates ----------------------------------------------------------

pub async fn updates(host_id: i64) -> Result<Vec<StackUpdate>> {
    get(&format!("/hosts/{host_id}/updates")).await
}

/// Checks one stack now, rather than waiting for the hourly sweep.
pub async fn check_stack(id: i64) -> Result<UpdateStatus> {
    post_empty(&format!("/stacks/{id}/check")).await
}

pub async fn set_auto_apply(id: i64, enabled: bool) -> Result<AutoApply> {
    let response = request("PUT", &format!("{BASE}/stacks/{id}/auto-apply"))
        .json(&AutoApply { enabled })
        .map_err(|e| Error::network(&e.to_string()))?
        .send()
        .await
        .map_err(|e| Error::network(&e.to_string()))?;
    read(response).await
}

// ---- operations -------------------------------------------------------

pub async fn container_logs(host_id: i64, container: &str) -> Result<Logs> {
    get(&format!(
        "/hosts/{host_id}/containers/{}/logs",
        component(container)
    ))
    .await
}

pub async fn cleanup_preview(host_id: i64) -> Result<CleanupPreview> {
    get(&format!("/hosts/{host_id}/cleanup")).await
}

pub async fn run_cleanup(host_id: i64, scope: CleanupScope) -> Result<CleanupResult> {
    post(
        &format!("/hosts/{host_id}/cleanup"),
        &CleanupRequest { scope },
    )
    .await
}

pub async fn audit() -> Result<Vec<AuditEntry>> {
    get("/audit").await
}

pub async fn metrics_now() -> Result<Now> {
    get("/hosts/1/metrics/now").await
}

pub async fn metrics_series(target: &Target, range: Range) -> Result<Series> {
    get(&format!(
        "/hosts/1/metrics/series?subject={}&range={}",
        component(&target.to_param()),
        range.as_str()
    ))
    .await
}

pub async fn container_figures(project: &str, range: Range) -> Result<Vec<ContainerFigures>> {
    get(&format!(
        "/hosts/1/metrics/containers?project={}&range={}",
        component(project),
        range.as_str()
    ))
    .await
}

pub async fn sizing() -> Result<Vec<Recommendation>> {
    get("/hosts/1/sizing").await
}

/// Percent-encodes everything but unreserved characters, for a query value
/// or a path segment. Subjects carry names and paths, which may hold
/// anything a directory name can; a path segment holding `/`, `?` or `#`
/// would otherwise address another endpoint altogether.
#[must_use]
pub fn component(text: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{component, env_url};

    #[test]
    fn a_path_segment_stays_one_segment() {
        assert_eq!(env_url(3, "API_KEY"), "/api/v1/stacks/3/env/API_KEY");
        assert_eq!(
            env_url(3, "A/../B?x#y"),
            "/api/v1/stacks/3/env/A%2F..%2FB%3Fx%23y"
        );
    }

    #[test]
    fn a_component_is_percent_encoded() {
        assert_eq!(
            component("container:blog-web_1.x~"),
            "container%3Ablog-web_1.x~"
        );
        assert_eq!(
            component("disk:/host/disks/my media&co"),
            "disk%3A%2Fhost%2Fdisks%2Fmy%20media%26co"
        );
        assert_eq!(component("é"), "%C3%A9");
    }
}
