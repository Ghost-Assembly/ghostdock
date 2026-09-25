//! The tools, and how each becomes calls to GhostDock's own API.
//!
//! Every tool is a translation: arguments in, one or more API requests made
//! with the caller's token, a compact answer out. Permission checks, audit
//! records and revocation are the API's, unchanged. A tool is listed only
//! for tokens holding its permission; one that also reads something else
//! (following a deploy it started, say) says so when it cannot.
//!
//! Anything placed into a request path is checked first, so an argument can
//! never steer a call to a different endpoint.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use serde_json::{Value, json};
use shared::token::Permission;
use tower::ServiceExt as _;

const HOST: i64 = store::hosts::LOCAL_HOST_ID;
const WAIT_DEFAULT_SECS: u64 = 300;
const WAIT_MAX_SECS: u64 = 900;
const OUTPUT_TAIL_LINES: usize = 60;

pub(super) struct Tool {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    permission: Permission,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
    input: fn() -> Value,
}

fn no_arguments() -> Value {
    json!({ "type": "object", "properties": {} })
}

fn stack_only() -> Value {
    json!({
        "type": "object",
        "properties": { "stack": { "type": "string", "description": "The stack's name or numeric id." } },
        "required": ["stack"],
    })
}

fn operation() -> Value {
    json!({
        "type": "object",
        "properties": {
            "stack": { "type": "string", "description": "The stack's name or numeric id." },
            "wait": { "type": "boolean", "description": "Wait for the outcome and return it with the end of the output. Default true." },
            "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": WAIT_MAX_SECS, "description": "How long to wait. Default 300." },
        },
        "required": ["stack"],
    })
}

fn metrics_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "subject": { "type": "string", "description": "A container name, \"stack:<project>\" for a stack's containers together, or \"host\"." },
            "range": { "type": "string", "enum": ["1h", "24h", "7d", "30d", "1y"], "description": "How far back. Default 24h." },
        },
        "required": ["subject"],
    })
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "list_stacks",
        title: "List stacks",
        description: "Every compose stack on the host with its state (running, degraded, unhealthy, stopped, or no containers), container counts, and each container's status. Includes stacks GhostDock does not manage.",
        permission: Permission::HostView,
        read_only: true,
        destructive: false,
        idempotent: true,
        input: no_arguments,
    },
    Tool {
        name: "get_stack",
        title: "Get a stack",
        description: "One registered stack in detail: where its compose file comes from, its containers, whether an update is waiting, whether it auto-applies updates, and its recent deployments.",
        permission: Permission::HostView,
        read_only: true,
        destructive: false,
        idempotent: true,
        input: stack_only,
    },
    Tool {
        name: "get_compose",
        title: "Read a compose file",
        description: "The compose file of a registered stack, as GhostDock deploys it. May contain values pasted inline.",
        permission: Permission::ComposeRead,
        read_only: true,
        destructive: false,
        idempotent: true,
        input: stack_only,
    },
    Tool {
        name: "host_info",
        title: "Host information",
        description: "The Docker host: daemon version, running and total containers, image count, and any problem with how GhostDock itself is deployed.",
        permission: Permission::HostView,
        read_only: true,
        destructive: false,
        idempotent: true,
        input: no_arguments,
    },
    Tool {
        name: "get_metrics",
        title: "Resource figures",
        description: "CPU, memory, network and disk use of a container, a stack or the host over a range: average, 95th percentile and peak of each. CPU is in cores, memory in bytes, rates in bytes per second.",
        permission: Permission::HostView,
        read_only: true,
        destructive: false,
        idempotent: true,
        input: metrics_input,
    },
    Tool {
        name: "sizing_recommendations",
        title: "Sizing recommendations",
        description: "For every container seen in the last 30 days: CPU and memory limits its history supports, the evidence for them, problems such as OOM kills, throttling or no memory limit, and compose lines to paste. Nothing is suggested before 3 days of history.",
        permission: Permission::HostView,
        read_only: true,
        destructive: false,
        idempotent: true,
        input: no_arguments,
    },
    Tool {
        name: "list_updates",
        title: "List updates",
        description: "For every registered stack: whether an update is waiting and why (new commits, image digests that moved), when it was last checked, whether the check failed, and whether it auto-applies.",
        permission: Permission::HostView,
        read_only: true,
        destructive: false,
        idempotent: true,
        input: no_arguments,
    },
    Tool {
        name: "check_updates",
        title: "Check for updates",
        description: "Checks one stack for new commits and new image digests now, rather than waiting for the hourly check, and returns what it found.",
        permission: Permission::UpdatesCheck,
        read_only: false,
        destructive: false,
        idempotent: true,
        input: stack_only,
    },
    Tool {
        name: "set_auto_apply",
        title: "Turn auto-apply on or off",
        description: "Whether GhostDock deploys a stack's updates as soon as it finds them. Off means nothing is deployed until someone asks.",
        permission: Permission::UpdatesAutoApply,
        read_only: false,
        destructive: false,
        idempotent: true,
        input: || {
            json!({
                "type": "object",
                "properties": {
                    "stack": { "type": "string", "description": "The stack's name or numeric id." },
                    "enabled": { "type": "boolean" },
                },
                "required": ["stack", "enabled"],
            })
        },
    },
    Tool {
        name: "deploy_stack",
        title: "Deploy a stack",
        description: "Runs docker compose up for a stack: pulls images, applies the compose file and any new commit, and waits until services are running or healthy. A stack that starts and then fails its health check is reported as failed. By default waits for the outcome.",
        permission: Permission::StacksDeploy,
        read_only: false,
        destructive: false,
        idempotent: true,
        input: operation,
    },
    Tool {
        name: "restart_stack",
        title: "Restart a stack",
        description: "Restarts a stack's containers in place, without pulling or recreating them.",
        permission: Permission::StacksRestart,
        read_only: false,
        destructive: false,
        idempotent: false,
        input: operation,
    },
    Tool {
        name: "stop_stack",
        title: "Stop a stack",
        description: "Stops a stack's containers, leaving them in place to be started again by a deploy or restart.",
        permission: Permission::StacksStop,
        read_only: false,
        destructive: true,
        idempotent: true,
        input: operation,
    },
    Tool {
        name: "take_down_stack",
        title: "Take a stack down",
        description: "Runs docker compose down: removes the stack's containers and networks. Named volumes and the registration are kept, so a later deploy brings it back with its data.",
        permission: Permission::StacksTakeDown,
        read_only: false,
        destructive: true,
        idempotent: true,
        input: operation,
    },
    Tool {
        name: "get_deployment",
        title: "Get a deployment",
        description: "One deployment (deploy, restart, stop or take-down): whether it is running, succeeded or failed, its exit code, and the end of what docker compose printed.",
        permission: Permission::HostView,
        read_only: true,
        destructive: false,
        idempotent: true,
        input: || {
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "The deployment's id." },
                    "lines": { "type": "integer", "minimum": 1, "maximum": 2000, "description": "How much of the end of the output to return. Default 100." },
                },
                "required": ["id"],
            })
        },
    },
    Tool {
        name: "container_logs",
        title: "Read container logs",
        description: "The latest output of one container, stdout and stderr marked apart, optionally only lines containing some text or only stderr.",
        permission: Permission::LogsView,
        read_only: true,
        destructive: false,
        idempotent: true,
        input: || {
            json!({
                "type": "object",
                "properties": {
                    "container": { "type": "string", "description": "The container's name or id." },
                    "lines": { "type": "integer", "minimum": 1, "maximum": 5000, "description": "How many of the latest lines to read. Default 200." },
                    "contains": { "type": "string", "description": "Only lines containing this text, ignoring case." },
                    "errors_only": { "type": "boolean", "description": "Only lines written to stderr." },
                },
                "required": ["container"],
            })
        },
    },
    Tool {
        name: "run_command",
        title: "Run a command in a container",
        description: "Runs one command with sh -c inside a running container and returns its output and exit code. As powerful as a shell on the host. Waits up to the timeout; a command still running then is left to finish.",
        permission: Permission::ShellOpen,
        read_only: false,
        destructive: true,
        idempotent: false,
        input: || {
            json!({
                "type": "object",
                "properties": {
                    "container": { "type": "string", "description": "The container's name or id." },
                    "command": { "type": "string", "description": "Run with sh -c, so pipes and redirections work." },
                    "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 300, "description": "Default 30." },
                },
                "required": ["container", "command"],
            })
        },
    },
    Tool {
        name: "list_env",
        title: "List environment variables",
        description: "The names of a stack's environment variables. Values are never returned by GhostDock.",
        permission: Permission::HostView,
        read_only: true,
        destructive: false,
        idempotent: true,
        input: stack_only,
    },
    Tool {
        name: "set_env",
        title: "Set an environment variable",
        description: "Sets one environment variable for a stack, stored encrypted. It cannot be read back. Takes effect at the next deploy.",
        permission: Permission::EnvWrite,
        read_only: false,
        destructive: false,
        idempotent: true,
        input: || {
            json!({
                "type": "object",
                "properties": {
                    "stack": { "type": "string", "description": "The stack's name or numeric id." },
                    "key": { "type": "string", "description": "Letters, digits and underscores, not starting with a digit." },
                    "value": { "type": "string" },
                },
                "required": ["stack", "key", "value"],
            })
        },
    },
    Tool {
        name: "delete_env",
        title: "Remove an environment variable",
        description: "Removes one environment variable from a stack. Takes effect at the next deploy.",
        permission: Permission::EnvWrite,
        read_only: false,
        destructive: true,
        idempotent: true,
        input: || {
            json!({
                "type": "object",
                "properties": {
                    "stack": { "type": "string", "description": "The stack's name or numeric id." },
                    "key": { "type": "string" },
                },
                "required": ["stack", "key"],
            })
        },
    },
    Tool {
        name: "create_stack",
        title: "Register a stack",
        description: "Registers a new stack from a compose file given in full. Nothing is deployed until deploy_stack is called. The name also becomes the compose project name.",
        permission: Permission::StacksCreate,
        read_only: false,
        destructive: false,
        idempotent: false,
        input: || {
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "compose_yaml": { "type": "string", "description": "The whole compose file." },
                },
                "required": ["name", "compose_yaml"],
            })
        },
    },
    Tool {
        name: "update_compose",
        title: "Change a compose file",
        description: "Replaces the compose file of a stack kept in GhostDock (not one from a Git repository, which changes by commit). Takes effect at the next deploy.",
        permission: Permission::StacksEdit,
        read_only: false,
        destructive: false,
        idempotent: true,
        input: || {
            json!({
                "type": "object",
                "properties": {
                    "stack": { "type": "string", "description": "The stack's name or numeric id." },
                    "compose_yaml": { "type": "string", "description": "The whole new compose file." },
                },
                "required": ["stack", "compose_yaml"],
            })
        },
    },
    Tool {
        name: "forget_stack",
        title: "Forget a stack",
        description: "Removes a stack's registration from GhostDock. Whatever it has running keeps running.",
        permission: Permission::StacksForget,
        read_only: false,
        destructive: true,
        idempotent: false,
        input: stack_only,
    },
    Tool {
        name: "cleanup_preview",
        title: "Preview cleanup",
        description: "What cleanup could remove: untagged images, images no container uses, and stopped containers nothing manages, with sizes.",
        permission: Permission::HostView,
        read_only: true,
        destructive: false,
        idempotent: true,
        input: no_arguments,
    },
    Tool {
        name: "cleanup",
        title: "Clean up",
        description: "Removes one group from the cleanup preview: dangling (untagged images), all_unused (every image no container uses), leftover (stopped containers from compose projects that are gone), or standalone (stopped containers not from compose). Volumes are never removed.",
        permission: Permission::CleanupRun,
        read_only: false,
        destructive: true,
        idempotent: false,
        input: || {
            json!({
                "type": "object",
                "properties": {
                    "scope": { "type": "string", "enum": ["dangling", "all_unused", "leftover", "standalone"] },
                },
                "required": ["scope"],
            })
        },
    },
    Tool {
        name: "activity",
        title: "Recent activity",
        description: "The audit trail, newest first: who did what and when, including actions taken through API tokens.",
        permission: Permission::ActivityView,
        read_only: true,
        destructive: false,
        idempotent: true,
        input: || {
            json!({
                "type": "object",
                "properties": { "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Default 50." } },
            })
        },
    },
    Tool {
        name: "list_repos",
        title: "List repositories",
        description: "The Git repositories stacks are deployed from, and the name of the credential each uses.",
        permission: Permission::HostView,
        read_only: true,
        destructive: false,
        idempotent: true,
        input: no_arguments,
    },
    Tool {
        name: "discover_stacks",
        title: "Find stacks in a repository",
        description: "Looks through a registered repository for compose files and says, for each, the name it would be registered under and whether it already is.",
        permission: Permission::StacksCreate,
        read_only: true,
        destructive: false,
        idempotent: true,
        input: || {
            json!({
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "The repository's URL or numeric id." },
                    "git_ref": { "type": "string", "description": "A full ref. Default refs/heads/main." },
                    "pattern": { "type": "string", "description": "Glob of paths to look for. Default: compose.yaml or docker-compose.yml in any folder, or any YAML in compose/." },
                },
                "required": ["repo"],
            })
        },
    },
    Tool {
        name: "import_stacks",
        title: "Register stacks from a repository",
        description: "Registers compose files found by discover_stacks as Git-backed stacks. Only files discovery offers as new are registered. Nothing is deployed.",
        permission: Permission::StacksCreate,
        read_only: false,
        destructive: false,
        idempotent: false,
        input: || {
            json!({
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "The repository's URL or numeric id." },
                    "git_ref": { "type": "string", "description": "A full ref. Default refs/heads/main." },
                    "pattern": { "type": "string" },
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Paths from discover_stacks." },
                },
                "required": ["repo", "paths"],
            })
        },
    },
];

pub(super) fn listed(permissions: &[Permission]) -> Vec<Value> {
    TOOLS
        .iter()
        .filter(|t| permissions.contains(&t.permission))
        .map(|t| {
            json!({
                "name": t.name,
                "title": t.title,
                "description": t.description,
                "inputSchema": (t.input)(),
                "annotations": {
                    "title": t.title,
                    "readOnlyHint": t.read_only,
                    "destructiveHint": t.destructive,
                    "idempotentHint": t.idempotent,
                    "openWorldHint": false,
                },
            })
        })
        .collect()
}

pub(super) fn find(name: &str, permissions: &[Permission]) -> Option<&'static Tool> {
    TOOLS
        .iter()
        .find(|t| t.name == name && permissions.contains(&t.permission))
}

/// GhostDock's own API, called in process with the caller's token.
pub(super) struct Api {
    router: Router,
    bearer: String,
    wake: Wake,
}

/// What wakes a wait for an operation's outcome, in place of asking over
/// and over. It tells a tool only when to look again: what it reports is
/// always read through the API with the caller's token.
pub(super) struct Wake {
    pub runner: crate::runner::Runner,
    pub revocations: crate::revocation::Revocations,
    pub principal: crate::auth::Principal,
}

impl Api {
    pub(super) fn new(router: Router, bearer: String, wake: Wake) -> Self {
        Self {
            router,
            bearer,
            wake,
        }
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let req = Request::builder()
            .method(method)
            .uri(format!("/api/v1{path}"))
            .header("authorization", format!("Bearer {}", self.bearer))
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .map_err(|e| format!("could not build the request: {e}"))?;
        let res = self
            .router
            .clone()
            .oneshot(req)
            .await
            .map_err(|e| format!("GhostDock could not answer: {e}"))?;
        let status = res.status();
        // Generous: the largest answers are logs, capped far below this.
        let bytes = axum::body::to_bytes(res.into_body(), 64 * 1024 * 1024)
            .await
            .map_err(|e| format!("GhostDock's answer broke off: {e}"))?;
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if status.is_success() {
            Ok(value)
        } else {
            let message = value["message"].as_str().unwrap_or("request failed");
            Err(format!("{message} (HTTP {})", status.as_u16()))
        }
    }

    async fn get(&self, path: &str) -> Result<Value, String> {
        self.request("GET", path, None).await
    }
}

// ---- arguments --------------------------------------------------------------

fn text<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("missing argument: {name}"))
}

fn number(args: &Value, name: &str, default: u64, max: u64) -> u64 {
    args.get(name)
        .and_then(Value::as_u64)
        .unwrap_or(default)
        .clamp(1, max)
}

/// A container name or id, which goes into a path.
fn container(args: &Value) -> Result<&str, String> {
    let name = text(args, "container")?;
    if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        Ok(name)
    } else {
        Err(format!("not a container name: {name}"))
    }
}

/// What `get_metrics` reads, as the series query's `subject`: a bare name
/// is a container's. Only characters names use, so it cannot carry a query
/// of its own.
fn metrics_subject(args: &Value) -> Result<String, String> {
    let subject = text(args, "subject")?;
    let target = if subject == "host" || subject.contains(':') {
        subject.to_owned()
    } else {
        format!("container:{subject}")
    };
    let safe = target
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | ':'))
        && !target.contains("..");
    match shared::metrics::Target::parse(&target) {
        Some(t) if safe => Ok(t.to_param().replace('/', "%2F").replace(':', "%3A")),
        _ => Err(format!(
            "not a subject: {subject}; use a container name, stack:<project>, or host"
        )),
    }
}

fn metrics_range(args: &Value) -> Result<shared::metrics::Range, String> {
    let text = args.get("range").and_then(Value::as_str).unwrap_or("24h");
    shared::metrics::Range::parse(text)
        .ok_or_else(|| format!("not a range: {text}; use 1h, 24h, 7d, 30d or 1y"))
}

/// An environment variable name, which goes into a path.
fn env_key(args: &Value) -> Result<&str, String> {
    let key = text(args, "key")?;
    let mut chars = key.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if first_ok && chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Ok(key)
    } else {
        Err(format!(
            "not a variable name: {key}; use letters, digits and underscores, not starting with a digit"
        ))
    }
}

/// A stack by name, project name or id.
struct Named {
    id: i64,
    /// The stack's entry from the updates list, when it was looked up there.
    entry: Option<Value>,
}

async fn stack(api: &Api, args: &Value) -> Result<Named, String> {
    let wanted = text(args, "stack")?;
    if let Ok(id) = wanted.parse::<i64>() {
        return Ok(Named { id, entry: None });
    }
    let list = api
        .get(&format!("/hosts/{HOST}/updates"))
        .await
        .map_err(|e| format!("could not look up stacks by name ({e}); use the numeric id"))?;
    list.as_array()
        .into_iter()
        .flatten()
        .find(|u| {
            u["stack"]["slug"].as_str() == Some(wanted)
                || u["stack"]["name"]
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(wanted))
        })
        .map(|u| Named {
            id: u["stack"]["id"].as_i64().unwrap_or_default(),
            entry: Some(u.clone()),
        })
        .ok_or_else(|| format!("no registered stack called {wanted}; list_stacks shows them"))
}

async fn repo(api: &Api, args: &Value) -> Result<i64, String> {
    let wanted = text(args, "repo")?;
    if let Ok(id) = wanted.parse::<i64>() {
        return Ok(id);
    }
    let repos = api.get("/repos").await?;
    repos
        .as_array()
        .into_iter()
        .flatten()
        .find(|r| r["url"].as_str() == Some(wanted))
        .and_then(|r| r["id"].as_i64())
        .ok_or_else(|| format!("no registered repository at {wanted}; list_repos shows them"))
}

// ---- running ------------------------------------------------------------------

pub(super) async fn run(tool: &Tool, api: &Api, args: &Value) -> Result<Value, String> {
    match tool.name {
        "list_stacks" => list_stacks(api).await,
        "get_stack" => get_stack(api, args).await,
        "get_compose" => {
            let s = stack(api, args).await?;
            let compose = api.get(&format!("/stacks/{}/compose", s.id)).await?;
            Ok(compose["compose_yaml"].clone())
        }
        "host_info" => api.get(&format!("/hosts/{HOST}")).await,
        "get_metrics" => {
            let (subject, range) = (metrics_subject(args)?, metrics_range(args)?);
            let series = api
                .get(&format!(
                    "/hosts/{HOST}/metrics/series?subject={subject}&range={}",
                    range.as_str()
                ))
                .await?;
            Ok(summarise(&series))
        }
        "sizing_recommendations" => Ok(named(
            "recommendations",
            api.get(&format!("/hosts/{HOST}/sizing")).await?,
        )),
        "list_updates" => list_updates(api).await,
        "check_updates" => {
            let s = stack(api, args).await?;
            api.request("POST", &format!("/stacks/{}/check", s.id), None)
                .await
        }
        "set_auto_apply" => {
            let s = stack(api, args).await?;
            let enabled = args
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or("missing argument: enabled")?;
            api.request(
                "PUT",
                &format!("/stacks/{}/auto-apply", s.id),
                Some(json!({ "enabled": enabled })),
            )
            .await
        }
        "deploy_stack" => operate(api, args, "deploy").await,
        "restart_stack" => operate(api, args, "restart").await,
        "stop_stack" => operate(api, args, "stop").await,
        "take_down_stack" => operate(api, args, "down").await,
        "get_deployment" => {
            let id = args
                .get("id")
                .and_then(Value::as_i64)
                .ok_or("missing argument: id")?;
            let lines = usize::try_from(number(args, "lines", 100, 2000)).unwrap_or(100);
            let detail = api.get(&format!("/deployments/{id}")).await?;
            Ok(outcome(&detail, lines))
        }
        "container_logs" => container_logs(api, args).await,
        "run_command" => run_command(api, args).await,
        "list_env" => {
            let s = stack(api, args).await?;
            api.get(&format!("/stacks/{}/env", s.id)).await
        }
        "set_env" => {
            let s = stack(api, args).await?;
            let key = env_key(args)?;
            let value = args
                .get("value")
                .and_then(Value::as_str)
                .ok_or("missing argument: value")?;
            api.request(
                "PUT",
                &format!("/stacks/{}/env/{key}", s.id),
                Some(json!({ "value": value })),
            )
            .await
        }
        "delete_env" => {
            let s = stack(api, args).await?;
            let key = env_key(args)?;
            api.request("DELETE", &format!("/stacks/{}/env/{key}", s.id), None)
                .await
        }
        "create_stack" => {
            let name = text(args, "name")?;
            let yaml = text(args, "compose_yaml")?;
            api.request(
                "POST",
                &format!("/hosts/{HOST}/stacks"),
                Some(json!({ "name": name, "compose_yaml": yaml })),
            )
            .await
        }
        "update_compose" => {
            let s = stack(api, args).await?;
            let yaml = text(args, "compose_yaml")?;
            // The route changes only the file and ignores the name, so the
            // stack is not read first: that would need host.view, which a
            // token allowed only to edit need not have.
            let name = s
                .entry
                .as_ref()
                .and_then(|e| e["stack"]["name"].as_str())
                .unwrap_or_default();
            api.request(
                "PUT",
                &format!("/stacks/{}", s.id),
                Some(json!({ "name": name, "compose_yaml": yaml })),
            )
            .await
        }
        "forget_stack" => {
            let s = stack(api, args).await?;
            api.request("DELETE", &format!("/stacks/{}", s.id), None)
                .await?;
            Ok(Value::String(format!(
                "stack {} is no longer registered",
                s.id
            )))
        }
        "cleanup_preview" => api.get(&format!("/hosts/{HOST}/cleanup")).await,
        "cleanup" => {
            let scope = text(args, "scope")?;
            api.request(
                "POST",
                &format!("/hosts/{HOST}/cleanup"),
                Some(json!({ "scope": scope })),
            )
            .await
        }
        "activity" => {
            let limit = usize::try_from(number(args, "limit", 50, 200)).unwrap_or(50);
            let entries = api.get("/audit").await?;
            Ok(named(
                "entries",
                Value::Array(
                    entries
                        .as_array()
                        .into_iter()
                        .flatten()
                        .take(limit)
                        .cloned()
                        .collect(),
                ),
            ))
        }
        "list_repos" => Ok(named("repositories", api.get("/repos").await?)),
        "discover_stacks" => {
            let id = repo(api, args).await?;
            api.request(
                "POST",
                &format!("/repos/{id}/discover"),
                Some(git_args(args)),
            )
            .await
        }
        "import_stacks" => {
            let id = repo(api, args).await?;
            let mut body = git_args(args);
            body["paths"] = args.get("paths").cloned().unwrap_or_else(|| json!([]));
            api.request("POST", &format!("/repos/{id}/import"), Some(body))
                .await
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

/// A series as a model can reason with it: average, 95th percentile and
/// peak, not three hundred points.
fn summarise(series: &Value) -> Value {
    let points = series["points"].as_array().cloned().unwrap_or_default();
    let stats = |avg: &str, peak: &str| {
        let mut values: Vec<f64> = points.iter().filter_map(|p| p[avg].as_f64()).collect();
        if values.is_empty() {
            return Value::Null;
        }
        values.sort_by(f64::total_cmp);
        #[allow(clippy::cast_precision_loss)]
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        let rank = domain::sizing::rank(values.len() as u64, 0.95);
        let p95 = usize::try_from(rank)
            .ok()
            .and_then(|i| values.get(i).copied())
            .unwrap_or(0.0);
        let max = points
            .iter()
            .filter_map(|p| p[peak].as_f64().or(p[avg].as_f64()))
            .fold(0.0, f64::max);
        json!({ "average": mean, "p95": p95, "peak": max })
    };
    json!({
        "subject": series["target"],
        "range": series["range"],
        "points": points.len(),
        "cpu": stats("cpu", "cpu_max"),
        "memory": stats("mem", "mem_max"),
        "network_in": stats("net_rx", "net_rx"),
        "network_out": stats("net_tx", "net_tx"),
        "disk_read": stats("io_read", "io_read"),
        "disk_write": stats("io_write", "io_write"),
        "note": "cpu in cores, memory in bytes, rates in bytes per second",
    })
}

/// A list as an object, so it has a name wherever it is read.
fn named(name: &str, list: Value) -> Value {
    json!({ name: list })
}

fn git_args(args: &Value) -> Value {
    json!({
        "git_ref": args.get("git_ref").and_then(Value::as_str).unwrap_or("refs/heads/main"),
        "pattern": args.get("pattern").and_then(Value::as_str),
    })
}

async fn list_stacks(api: &Api) -> Result<Value, String> {
    let board = api.get(&format!("/hosts/{HOST}/stacks")).await?;
    Ok(named("stacks", Value::Array(
        board
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| {
                json!({
                    "name": s["managed"]["name"].as_str().or(s["project"].as_str()),
                    "project": s["project"],
                    "id": s["managed"]["id"],
                    "registered": !s["managed"].is_null(),
                    "state": s["state"],
                    "running": s["running_count"],
                    "containers": s["containers"].as_array().into_iter().flatten().map(|c| json!({
                        "name": c["name"], "image": c["image"], "state": c["state"],
                        "status": c["status"], "health": c["health"],
                    })).collect::<Vec<_>>(),
                    "busy": s["managed"]["busy"],
                })
            })
            .collect(),
    )))
}

async fn list_updates(api: &Api) -> Result<Value, String> {
    let list = api.get(&format!("/hosts/{HOST}/updates")).await?;
    Ok(named(
        "updates",
        Value::Array(
            list.as_array()
                .into_iter()
                .flatten()
                .map(|u| {
                    json!({
                        "stack": u["stack"]["name"],
                        "id": u["stack"]["id"],
                        "update": u["reason"],
                        "checked_at": u["status"]["checked_at"],
                        "check_failed": u["status"]["error"],
                        "auto_apply": u["auto_apply"],
                        "busy": u["busy"],
                    })
                })
                .collect(),
        ),
    ))
}

async fn get_stack(api: &Api, args: &Value) -> Result<Value, String> {
    let s = stack(api, args).await?;
    let registration = api.get(&format!("/stacks/{}", s.id)).await?;
    let history = api.get(&format!("/stacks/{}/deployments", s.id)).await?;
    let entry = match s.entry {
        Some(entry) => Some(entry),
        None => api
            .get(&format!("/hosts/{HOST}/updates"))
            .await
            .ok()
            .and_then(|list| {
                list.as_array()?
                    .iter()
                    .find(|u| u["stack"]["id"].as_i64() == Some(s.id))
                    .cloned()
            }),
    };
    // Containers come from the daemon; the rest is still worth having when
    // it cannot be reached.
    let containers = match api.get(&format!("/hosts/{HOST}/stacks")).await {
        Ok(board) => board
            .as_array()
            .into_iter()
            .flatten()
            .find(|b| b["project"] == registration["slug"])
            .map_or_else(|| json!([]), |b| b["containers"].clone()),
        Err(e) => Value::String(format!("unavailable: {e}")),
    };
    Ok(json!({
        "id": registration["id"],
        "name": registration["name"],
        "project": registration["slug"],
        "source": registration["source_kind"],
        "git": registration["git"],
        "update": entry.as_ref().map(|e| e["reason"].clone()),
        "last_checked": entry.as_ref().map(|e| e["status"]["checked_at"].clone()),
        "auto_apply": entry.as_ref().map(|e| e["auto_apply"].clone()),
        "containers": containers,
        "recent_deployments": history.as_array().into_iter().flatten().take(5).cloned().collect::<Vec<_>>(),
    }))
}

/// Starts an operation and, unless told not to, follows it to the end.
///
/// Between reads of the deployment, which go through the API, it sleeps
/// until the operation is announced finished, the token is revoked, or
/// [`RECHECK`] passes, whichever is first.
async fn operate(api: &Api, args: &Value, action: &str) -> Result<Value, String> {
    let s = stack(api, args).await?;
    // Before starting, so an operation that ends at once is not missed.
    let mut finished = api.wake.runner.subscribe();
    let revoked = api.wake.revocations.until_revoked(&api.wake.principal);
    let started = api
        .request("POST", &format!("/stacks/{}/{action}", s.id), None)
        .await?;
    let Some(id) = started["id"].as_i64() else {
        return Ok(started);
    };
    if args.get("wait").and_then(Value::as_bool) == Some(false) {
        return Ok(json!({
            "deployment_id": id,
            "status": "running",
            "note": "started; get_deployment follows it",
        }));
    }
    let limit = Duration::from_secs(number(
        args,
        "timeout_seconds",
        WAIT_DEFAULT_SECS,
        WAIT_MAX_SECS,
    ));
    let deadline = tokio::time::Instant::now() + limit;
    tokio::pin!(revoked);
    let mut watching_revocation = true;
    loop {
        // A revoked token fails here, as it would anywhere in the API.
        let detail = match api.get(&format!("/deployments/{id}")).await {
            Ok(detail) => detail,
            Err(e) => {
                return Ok(json!({
                    "deployment_id": id,
                    "status": "running",
                    "note": format!("started, but could not be followed ({e}); following needs host.view"),
                }));
            }
        };
        if detail["status"] != json!("running") {
            return Ok(outcome(&detail, OUTPUT_TAIL_LINES));
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            let mut still = outcome(&detail, OUTPUT_TAIL_LINES);
            still["note"] = json!("still running when the wait ran out; get_deployment follows it");
            return Ok(still);
        }
        tokio::select! {
            () = announced(&mut finished, id) => {}
            // Read again at once, and let the API refuse. Falling behind
            // the announcements also ends this, as a missed revocation
            // might have been among them; if the token still works, the
            // wait goes on, read every RECHECK.
            () = &mut revoked, if watching_revocation => watching_revocation = false,
            () = tokio::time::sleep_until(deadline.min(now + RECHECK)) => {}
        }
    }
}

/// Longest sleep between reads of a running operation. Its end is
/// announced, so this only bounds the wait should that announcement never
/// come.
const RECHECK: Duration = Duration::from_secs(30);

/// Returns once deployment `id` is announced finished, or once events were
/// missed, which may have included that.
async fn announced(
    events: &mut tokio::sync::broadcast::Receiver<shared::event::ServerEvent>,
    id: i64,
) {
    use shared::event::ServerEvent;
    use tokio::sync::broadcast::error::RecvError;
    loop {
        match events.recv().await {
            Ok(ServerEvent::DeploymentFinished { deployment }) if deployment.id == id => return,
            Ok(_) => {}
            Err(RecvError::Lagged(_)) => return,
            // Nothing more will be announced; the recheck carries on.
            Err(RecvError::Closed) => std::future::pending().await,
        }
    }
}

/// A deployment reduced to what an operator reads: how it ended, and the
/// end of what compose said, which is where the explanation is.
fn outcome(detail: &Value, lines: usize) -> Value {
    let log = detail["log"].as_str().unwrap_or_default();
    let all: Vec<&str> = log.lines().collect();
    let tail = all
        .iter()
        .skip(all.len().saturating_sub(lines))
        .copied()
        .collect::<Vec<_>>();
    json!({
        "deployment_id": detail["id"],
        "stack_id": detail["stack_id"],
        "action": detail["action"],
        "status": detail["status"],
        "exit_code": detail["exit_code"],
        "commit": detail["commit_sha"],
        "started_at": detail["started_at"],
        "finished_at": detail["finished_at"],
        "output": tail.join("\n"),
    })
}

async fn container_logs(api: &Api, args: &Value) -> Result<Value, String> {
    let name = container(args)?;
    let tail = number(args, "lines", 200, 5000);
    let logs = api
        .get(&format!("/hosts/{HOST}/containers/{name}/logs?tail={tail}"))
        .await?;
    let needle = args
        .get("contains")
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    let errors_only = args.get("errors_only").and_then(Value::as_bool) == Some(true);
    let lines: Vec<String> = logs["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|l| !errors_only || l["stream"] == json!("stderr"))
        .filter_map(|l| {
            let text = l["text"].as_str()?;
            if needle
                .as_ref()
                .is_some_and(|n| !text.to_lowercase().contains(n))
            {
                return None;
            }
            let marker = if l["stream"] == json!("stderr") {
                "err"
            } else {
                "out"
            };
            Some(format!(
                "{marker} {} {text}",
                l["at"].as_str().unwrap_or("")
            ))
        })
        .collect();
    let header = format!(
        "{} lines{}{}",
        lines.len(),
        if logs["truncated"] == json!(true) {
            " (older output exists; raise lines to see more)"
        } else {
            ""
        },
        if needle.is_some() || errors_only {
            ", after filtering"
        } else {
            ""
        },
    );
    Ok(Value::String(format!("{header}\n{}", lines.join("\n"))))
}

async fn run_command(api: &Api, args: &Value) -> Result<Value, String> {
    let name = container(args)?;
    let command = text(args, "command")?;
    let timeout = args.get("timeout_seconds").and_then(Value::as_u64);
    let result = api
        .request(
            "POST",
            &format!("/hosts/{HOST}/containers/{name}/exec/run"),
            Some(json!({ "command": command, "timeout_seconds": timeout })),
        )
        .await?;
    let output: Vec<String> = result["output"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| {
            let text = l["text"].as_str()?;
            Some(if l["stream"] == json!("stderr") {
                format!("[stderr] {text}")
            } else {
                text.to_owned()
            })
        })
        .collect();
    Ok(json!({
        "exit_code": result["exit_code"],
        "timed_out": result["timed_out"],
        "truncated": result["truncated"],
        "output": output.join("\n"),
    }))
}
