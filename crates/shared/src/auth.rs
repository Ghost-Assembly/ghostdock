//! Authentication wire types.

use serde::{Deserialize, Serialize};

/// Whether the instance has an admin yet, and who the caller is.
///
/// Served unauthenticated so the client knows whether to show the login
/// form or the first-run setup form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthStatus {
    /// False until the first admin account exists.
    pub bootstrapped: bool,
    /// The current user, if the request carried a valid session.
    pub user: Option<User>,
}

/// An authenticated user, as exposed to clients.
///
/// Deliberately carries no password material of any kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: String,
}

/// Credentials for logging in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// An account, as listed on the accounts screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: i64,
    pub username: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Whether this is the caller's own account, which they cannot remove.
    pub you: bool,
}

/// Replacing your own password.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasswordChange {
    pub current: String,
    pub new: String,
}
