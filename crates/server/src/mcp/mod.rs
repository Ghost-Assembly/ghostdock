//! MCP, the Model Context Protocol: GhostDock as a set of tools an AI
//! assistant can use, over the Streamable HTTP transport Claude Code speaks.
//!
//! Owned rather than taken from an SDK. Offering tools needs four methods
//! (`initialize`, `ping`, `tools/list`, `tools/call`) over plain
//! request-and-reply JSON, which is small and stable enough to hold in full
//! here, where the SDKs move quickly.
//!
//! Stateless: no session is issued, every request is authenticated afresh,
//! and each reply is a single JSON body. A revoked token stops working on
//! its next request, and there is no stream to close.
//!
//! Tools reach GhostDock through its own API, carrying the caller's token (see
//! [`tools`]). What a tool may do is therefore decided by exactly the rules
//! the API enforces, not by a second copy of them here.

mod tools;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::{Value, json};
use shared::token::Permission;

use crate::auth::{Principal, Via};
use crate::state::AppState;

/// Newest first. The first is offered to a client asking for one it does not
/// know; `2025-03-26` is the first with this transport.
const VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];

const INSTRUCTIONS: &str = "GhostDock manages Docker Compose stacks on one host. \
Refer to a stack by its name or id; list_stacks shows them. Deploying pulls images \
and waits until services are running or healthy, so a deploy that reports success \
means the stack came up. Take-down removes containers and networks but keeps named \
volumes. Environment values can be set but never read back. What you can do depends \
on the permissions of the token you were given.";

#[derive(Clone)]
struct Mcp {
    state: AppState,
    /// The API, which tools call into. Not the whole app: /mcp is not in it,
    /// so a tool can never call back into MCP.
    api: Router,
}

pub fn routes(state: AppState, api: Router) -> Router {
    Router::new()
        .route("/mcp", post(handle).get(not_offered).delete(not_offered))
        .with_state(Mcp { state, api })
}

/// No server-initiated stream is offered, and there is no session to end.
async fn not_offered() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "POST")],
        axum::Json(rpc_error(
            Value::Null,
            -32000,
            "this server answers POST only",
        )),
    )
        .into_response()
}

/// The caller, as far as MCP needs to know it.
pub(crate) struct Caller {
    pub bearer: String,
    pub permissions: Vec<Permission>,
    /// Who the token is, as the API sees it: for ending a wait when the
    /// token is revoked.
    pub principal: Principal,
}

async fn handle(State(mcp): State<Mcp>, headers: HeaderMap, body: Bytes) -> Response {
    let get = |name: header::HeaderName| headers.get(name).and_then(|v| v.to_str().ok());

    // DNS rebinding: a page elsewhere must not reach this through a browser.
    if !crate::origin::is_allowed(
        get(header::ORIGIN),
        get(header::HOST),
        &mcp.state.allowed_origins,
    ) {
        return reply(
            StatusCode::FORBIDDEN,
            rpc_error(Value::Null, -32000, "origin not allowed"),
        );
    }

    let caller = match authenticate(&mcp.state, get(header::AUTHORIZATION)).await {
        Some(caller) => caller,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Bearer realm=\"ghostdock\"")],
                axum::Json(rpc_error(
                    Value::Null,
                    -32000,
                    "send a GhostDock API token as Authorization: Bearer",
                )),
            )
                .into_response();
        }
    };

    if !get(header::CONTENT_TYPE).is_some_and(|v| v.starts_with("application/json")) {
        return reply(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            rpc_error(Value::Null, -32000, "Content-Type must be application/json"),
        );
    }
    let accepts = get(header::ACCEPT).unwrap_or("*/*");
    if !["application/json", "*/*", "application/*"]
        .iter()
        .any(|t| accepts.contains(t))
    {
        return reply(
            StatusCode::NOT_ACCEPTABLE,
            rpc_error(
                Value::Null,
                -32000,
                "this server replies with application/json",
            ),
        );
    }
    if let Some(version) = headers
        .get("mcp-protocol-version")
        .and_then(|v| v.to_str().ok())
        && !VERSIONS.contains(&version)
    {
        return reply(
            StatusCode::BAD_REQUEST,
            rpc_error(
                Value::Null,
                -32000,
                &format!(
                    "unsupported protocol version {version}; supported: {}",
                    VERSIONS.join(", ")
                ),
            ),
        );
    }

    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return reply(
            StatusCode::BAD_REQUEST,
            rpc_error(Value::Null, -32700, "parse error"),
        );
    };
    if message.is_array() {
        // Batching left the protocol in 2025-06-18.
        return reply(
            StatusCode::BAD_REQUEST,
            rpc_error(Value::Null, -32600, "batches are not supported"),
        );
    }
    let (Some(method), Some(id)) = (
        message.get("method").and_then(Value::as_str),
        message.get("id"),
    ) else {
        // A notification, or a response to a request this server never sends.
        return StatusCode::ACCEPTED.into_response();
    };
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

    let answer = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools::listed(&caller.permissions) })),
        "tools/call" => call(&mcp, &caller, &params).await,
        other => Err((-32601, format!("method not found: {other}"))),
    };
    let body = match answer {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => rpc_error(id.clone(), code, &message),
    };
    reply(StatusCode::OK, body)
}

/// A token, and only a token: a browser's session never reaches MCP.
async fn authenticate(state: &AppState, authorization: Option<&str>) -> Option<Caller> {
    let secret = crate::auth::bearer(authorization?)?;
    let principal = Principal::from_token(state, secret).await.ok()?;
    let Via::Token { permissions, .. } = &principal.via else {
        return None;
    };
    Some(Caller {
        bearer: secret.to_owned(),
        permissions: permissions.clone(),
        principal,
    })
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|v| VERSIONS.contains(v))
        .unwrap_or(VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "ghostdock",
            "title": "GhostDock",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    })
}

async fn call(mcp: &Mcp, caller: &Caller, params: &Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "tools/call needs a tool name".to_owned()))?;
    // A tool outside the token's permissions is not listed, so for this
    // caller it does not exist.
    let tool = tools::find(name, &caller.permissions)
        .ok_or_else(|| (-32602, format!("unknown tool: {name}")))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let api = tools::Api::new(
        mcp.api.clone(),
        caller.bearer.clone(),
        tools::Wake {
            runner: mcp.state.runner.clone(),
            revocations: mcp.state.revocations.clone(),
            principal: caller.principal.clone(),
        },
    );
    Ok(match tools::run(tool, &api, &arguments).await {
        // Text reads best as itself: logs, a compose file. Anything with
        // shape also goes as structuredContent, which clients prefer.
        Ok(Value::String(text)) => json!({
            "content": [{ "type": "text", "text": text }],
            "isError": false,
        }),
        Ok(value) => json!({
            "content": [{ "type": "text", "text": render(&value) }],
            "structuredContent": structured(value),
            "isError": false,
        }),
        // Reported as a result, so the model can read it and adjust.
        Err(message) => json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true,
        }),
    })
}

/// Text for a model to read: the same data, pretty-printed, or the string
/// itself when that is all there is.
fn render(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

/// `structuredContent` must be an object. Tools return lists named, as
/// objects; anything else that slips through is wrapped rather than lost.
fn structured(value: Value) -> Value {
    match value {
        Value::Object(_) => value,
        other => json!({ "result": other }),
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn reply(status: StatusCode, body: Value) -> Response {
    (status, axum::Json(body)).into_response()
}
