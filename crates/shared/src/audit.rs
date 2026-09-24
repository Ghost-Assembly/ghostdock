//! A record of what was done.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: i64,
    pub at: DateTime<Utc>,
    /// The name at the time, kept even if the account is later removed.
    pub username: String,
    pub action: String,
    pub target: String,
    pub detail: Option<String>,
}
