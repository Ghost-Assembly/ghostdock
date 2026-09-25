//! The single error boundary for the HTTP API.
//!
//! Every handler returns this, and it is the only place that decides a
//! status code or what a client is told. Internal detail is logged, never
//! serialised: an error message is not a place to leak schema or paths.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("authentication required")]
    NotAuthenticated,
    #[error("invalid username or password")]
    InvalidCredentials,
    #[error("{0}")]
    BadRequest(String),
    #[error("this request did not come from GhostDock's own pages")]
    Forbidden,
    #[error("the current password is not right")]
    WrongPassword,
    #[error("this needs a person signed in, not an API token")]
    SessionOnly,
    #[error("this token does not have the {} permission", .0.as_str())]
    MissingPermission(shared::token::Permission),
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("the Docker daemon is not reachable")]
    DaemonUnavailable,
    #[error("Too many failed sign-ins. Try again in a few minutes.")]
    TooManyAttempts,
    /// Anything unexpected. The cause is logged; the client is told nothing.
    #[error("internal error")]
    Internal(#[source] anyhow::Error),
}

impl ApiError {
    fn parts(&self) -> (StatusCode, &'static str) {
        match self {
            Self::NotAuthenticated => (StatusCode::UNAUTHORIZED, "not_authenticated"),
            // Deliberately indistinguishable from a wrong password, so this
            // endpoint cannot be used to discover which usernames exist.
            Self::InvalidCredentials => (StatusCode::UNAUTHORIZED, "invalid_credentials"),
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Self::WrongPassword => (StatusCode::FORBIDDEN, "wrong_password"),
            Self::SessionOnly => (StatusCode::FORBIDDEN, "session_only"),
            Self::MissingPermission(_) => (StatusCode::FORBIDDEN, "missing_permission"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            Self::DaemonUnavailable => (StatusCode::SERVICE_UNAVAILABLE, "daemon_unavailable"),
            Self::TooManyAttempts => (StatusCode::TOO_MANY_REQUESTS, "too_many_attempts"),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.parts();

        let message = match &self {
            Self::Internal(cause) => {
                tracing::error!(error = ?cause, "request failed");
                "an internal error occurred".to_owned()
            }
            other => other.to_string(),
        };

        (
            status,
            Json(shared::ApiError {
                code: code.to_owned(),
                message,
            }),
        )
            .into_response()
    }
}

impl From<store::Error> for ApiError {
    fn from(e: store::Error) -> Self {
        Self::Internal(anyhow::Error::new(e))
    }
}

impl From<docker::Error> for ApiError {
    fn from(e: docker::Error) -> Self {
        match e.kind() {
            // The daemon answered; the answer is about what was asked for.
            docker::ErrorKind::NotFound => Self::NotFound,
            docker::ErrorKind::Conflict => Self::Conflict(
                e.daemon_message()
                    .map_or_else(|| "Docker refused that.".to_owned(), str::to_owned),
            ),
            docker::ErrorKind::Unavailable => {
                tracing::warn!(error = ?e, "docker call failed");
                Self::DaemonUnavailable
            }
        }
    }
}

impl From<tower_sessions::session::Error> for ApiError {
    fn from(e: tower_sessions::session::Error) -> Self {
        Self::Internal(anyhow::Error::new(e))
    }
}
