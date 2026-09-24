//! Deciding whether a WebSocket handshake may proceed.
//!
//! WebSocket handshakes are not subject to the same-origin policy the way
//! `fetch` and `EventSource` are: any page anywhere can open a socket to
//! this server, and the browser will attach cookies for it. That makes the
//! exec endpoint -- a root shell inside a container -- reachable from a
//! malicious page unless the origin is checked.
//!
//! `SameSite=Lax` on the session cookie blocks this in current browsers, but
//! relying on it alone means one cookie-attribute change silently removes
//! the only protection on the most powerful endpoint in the product.

/// Whether a handshake carrying `origin` may be accepted by `host`.
///
/// An absent origin is allowed: browsers always send one on a WebSocket
/// handshake, so its absence means a non-browser client, which is not the
/// threat this guards against. A *present* origin must match.
#[must_use]
pub fn is_allowed(origin: Option<&str>, host: Option<&str>, allowed: &[String]) -> bool {
    let Some(origin) = origin else {
        return true;
    };

    // An explicit allowlist, for a reverse proxy that does not preserve the
    // original Host.
    if allowed.iter().any(|entry| entry == origin) {
        return true;
    }

    let Some(host) = host else {
        // No Host to compare against and no allowlist entry: nothing here
        // justifies accepting the socket.
        return false;
    };

    authority_of(origin).is_some_and(|authority| authority == host)
}

/// The `host:port` part of an origin, without its scheme.
///
/// Compared against the Host header, which carries a port only when it is
/// not the default -- and an origin is written the same way, so the two are
/// directly comparable without normalising ports.
fn authority_of(origin: &str) -> Option<&str> {
    let rest = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))?;

    // An origin has no path, but a malformed one might; refuse rather than
    // guess at what was meant.
    if rest.contains('/') || rest.is_empty() {
        return None;
    }
    Some(rest)
}

/// Origins accepted in addition to the server's own host.
///
/// Read from the environment by the caller, so this module stays pure.
#[must_use]
pub fn parse_allowlist(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect()
}

/// An extractor that admits only handshakes from GhostDock's own pages.
///
/// Written as an extractor rather than a check inside a handler so that it
/// runs during extraction, in argument order, before anything that could
/// grant an upgrade. A check buried in a handler body is one refactor away
/// from being skipped, and this one stands between a malicious page and a
/// root shell.
pub struct SameOrigin;

impl axum::extract::FromRequestParts<crate::state::AppState> for SameOrigin {
    type Rejection = crate::error::ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &crate::state::AppState,
    ) -> Result<Self, Self::Rejection> {
        use axum::http::header::{HOST, ORIGIN};

        let header = |name: axum::http::HeaderName| {
            parts
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };

        let origin = header(ORIGIN);
        if is_allowed(
            origin.as_deref(),
            header(HOST).as_deref(),
            &state.allowed_origins,
        ) {
            Ok(Self)
        } else {
            tracing::warn!(?origin, "refused a handshake from another origin");
            Err(crate::error::ApiError::Forbidden)
        }
    }
}
