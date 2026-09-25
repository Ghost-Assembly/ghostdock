//! Router assembly and the session layer.

use std::path::Path;

use axum::Router;
use tower_http::compression::CompressionLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_sessions::cookie::SameSite;
use tower_sessions::cookie::time::Duration;
use tower_sessions::{Expiry, SessionManagerLayer};

use crate::session_store::SqliteSessionStore;
use crate::state::AppState;

/// How long a session survives without use.
const SESSION_IDLE_DAYS: i64 = 7;

/// Cookie name. Prefixed so it cannot be confused with an application cookie
/// from something else sharing the host.
const SESSION_COOKIE: &str = "ghostdock.sid";

/// Builds the router.
///
/// `secure_cookies` should be true whenever GhostDock is reachable over HTTPS.
/// It is not the default because a great many deployments are plain HTTP on a
/// local network, where a Secure cookie is simply never sent and login fails
/// with no visible reason.
///
/// `ui` is the directory holding the built web client. When absent, GhostDock
/// serves the API alone — which is what the tests use, and is a legitimate way
/// to run it behind a separately hosted frontend.
pub fn build(state: AppState, secure_cookies: bool, ui: Option<&Path>) -> Router {
    let session_layer = SessionManagerLayer::new(SqliteSessionStore::new(state.store.clone()))
        .with_name(SESSION_COOKIE)
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_secure(secure_cookies)
        .with_expiry(Expiry::OnInactivity(Duration::days(SESSION_IDLE_DAYS)));

    // Every API route, each mounted with its entry in the reference.
    let api = crate::reference::api().into_router();

    let router = Router::new()
        .nest(crate::reference::API_PREFIX, api)
        .layer(session_layer)
        .with_state(state.clone());
    // MCP tools call into the API above, so they get a clone of it; /mcp is
    // added after, so a tool can never call back into MCP.
    let router = router.clone().merge(crate::mcp::routes(state, router));

    let router = match ui {
        // Unknown paths fall back to index.html so client-side routes survive
        // a refresh or a shared link; the API is nested above and matches first.
        // Precompressed variants are written at build time (see the web
        // crate's precompress.sh) and preferred when the client accepts them;
        // the compression layer below leaves already-encoded responses alone.
        Some(dir) => router.fallback_service(
            ServeDir::new(dir)
                .precompressed_br()
                .precompressed_gzip()
                .fallback(ServeFile::new(dir.join("index.html"))),
        ),
        None => router,
    };

    // Outermost, so it covers API responses, and the web client when it was
    // built without precompressed variants. Without it every cold load on a
    // phone pays the full uncompressed size.
    router.layer(CompressionLayer::new().br(true).gzip(true))
}
