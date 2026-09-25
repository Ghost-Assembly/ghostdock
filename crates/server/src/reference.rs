//! The API and MCP reference, generated from the code that serves them.
//!
//! Routes under `/api/v1` are declared through [`Routes`], which mounts each
//! handler and records its entry in the endpoint table in the same call, so a
//! route cannot be mounted without being documented. What an entry says
//! about access is checked against what its handler enforces by
//! `tests/reference.rs`, which also fails on any route mounted directly with
//! axum's `route` that the table does not list (`/mcp` is one, mounted
//! beside the API).

use std::sync::LazyLock;

use axum::handler::Handler;
use axum::routing::{MethodRouter, delete, get, post, put};
use axum::{Json, Router};
use shared::reference::{Access, Endpoint, PermissionInfo, Reference};
use shared::token::Permission;

use crate::auth::Authenticated;
use crate::state::AppState;

/// Where the API is nested.
pub const API_PREFIX: &str = "/api/v1";

/// Routes and their entries in the endpoint table, built together.
pub struct Routes {
    router: Router<AppState>,
    endpoints: Vec<Endpoint>,
    area: &'static str,
}

impl Routes {
    /// Routes whose entries are listed under `area` until [`Routes::area`]
    /// says otherwise.
    #[must_use]
    pub fn new(area: &'static str) -> Self {
        Self {
            router: Router::new(),
            endpoints: Vec::new(),
            area,
        }
    }

    /// Lists the entries that follow under `area`.
    #[must_use]
    pub fn area(mut self, area: &'static str) -> Self {
        self.area = area;
        self
    }

    #[must_use]
    pub fn get<H, T>(self, path: &str, access: Access, summary: &str, handler: H) -> Self
    where
        H: Handler<T, AppState>,
        T: 'static,
    {
        self.add("GET", path, access, summary, get(handler))
    }

    #[must_use]
    pub fn post<H, T>(self, path: &str, access: Access, summary: &str, handler: H) -> Self
    where
        H: Handler<T, AppState>,
        T: 'static,
    {
        self.add("POST", path, access, summary, post(handler))
    }

    #[must_use]
    pub fn put<H, T>(self, path: &str, access: Access, summary: &str, handler: H) -> Self
    where
        H: Handler<T, AppState>,
        T: 'static,
    {
        self.add("PUT", path, access, summary, put(handler))
    }

    #[must_use]
    pub fn delete<H, T>(self, path: &str, access: Access, summary: &str, handler: H) -> Self
    where
        H: Handler<T, AppState>,
        T: 'static,
    {
        self.add("DELETE", path, access, summary, delete(handler))
    }

    fn add(
        mut self,
        method: &str,
        path: &str,
        access: Access,
        summary: &str,
        route: MethodRouter<AppState>,
    ) -> Self {
        self.endpoints.push(Endpoint {
            method: method.to_owned(),
            path: format!("{API_PREFIX}{path}"),
            area: self.area.to_owned(),
            access,
            summary: summary.to_owned(),
        });
        // Axum merges a second method on a path already routed.
        self.router = self.router.route(path, route);
        self
    }

    #[must_use]
    pub fn merge(mut self, other: Self) -> Self {
        self.router = self.router.merge(other.router);
        self.endpoints.extend(other.endpoints);
        self
    }

    pub fn into_router(self) -> Router<AppState> {
        self.router
    }
}

/// Every route under `/api/v1`, with its entry in the table.
#[must_use]
pub fn api() -> Routes {
    Routes::new("Server")
        .get(
            "/health",
            Access::Public,
            "Answers ok while the server is up",
            || async { "ok" },
        )
        .get(
            "/reference",
            Access::Authenticated,
            "This reference: every endpoint, MCP tool and permission",
            serve,
        )
        .merge(crate::auth::routes())
        .merge(crate::accounts::routes())
        .merge(crate::tokens::routes())
        .merge(crate::hosts::routes())
        .merge(crate::stacks::routes())
        .merge(crate::sources::routes())
        .merge(crate::discovery::routes())
        .merge(crate::updates::routes())
        .merge(crate::ops::routes())
        .merge(crate::logs_across::routes())
        .merge(crate::exec::routes())
        .merge(crate::metrics::routes::routes())
        .merge(crate::checks::routes::routes())
        .merge(crate::alerts::routes::routes())
        .merge(crate::audit::routes())
        .merge(crate::events::routes())
}

/// The endpoint table: the API's, then those mounted beside it.
#[must_use]
pub fn endpoints() -> Vec<Endpoint> {
    let mut all = api().endpoints;
    all.extend(crate::mcp::endpoints());
    all
}

/// The whole reference.
#[must_use]
pub fn reference() -> Reference {
    Reference {
        endpoints: endpoints(),
        tools: crate::mcp::tools(),
        permissions: Permission::ALL
            .into_iter()
            .map(|permission| PermissionInfo {
                permission,
                area: permission.area().to_owned(),
                description: permission.describe().to_owned(),
            })
            .collect(),
    }
}

/// Built once: it describes the code, which does not change while running.
static REFERENCE: LazyLock<Reference> = LazyLock::new(reference);

/// Any caller who has proved who they are: it names what exists and who may
/// call it, never a secret.
async fn serve(_caller: Authenticated) -> Json<Reference> {
    Json(REFERENCE.clone())
}
