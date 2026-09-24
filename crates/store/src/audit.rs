//! Recording what was done.

use chrono::DateTime;
use shared::audit::AuditEntry;

use crate::{Result, Store};

impl Store {
    /// Records one action.
    ///
    /// Takes the username as well as the id, because the trail has to keep
    /// answering "who" after an account is removed.
    pub async fn audit(
        &self,
        user_id: Option<i64>,
        username: &str,
        action: &str,
        target: &str,
        detail: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO audit_log (at, user_id, username, action, target, detail)
             VALUES (unixepoch(), ?1, ?2, ?3, ?4, ?5)",
        )
        .bind(user_id)
        .bind(username)
        .bind(action)
        .bind(target)
        .bind(detail)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Most recent first.
    pub async fn audit_recent(&self, limit: i64) -> Result<Vec<AuditEntry>> {
        let rows = sqlx::query_as::<_, (i64, i64, String, String, String, Option<String>)>(
            "SELECT id, at, username, action, target, detail
             FROM audit_log ORDER BY id DESC LIMIT ?1",
        )
        .bind(limit)
        .fetch_all(self.pool())
        .await?;

        Ok(rows
            .into_iter()
            .map(|(id, at, username, action, target, detail)| AuditEntry {
                id,
                at: DateTime::from_timestamp(at, 0).unwrap_or_default(),
                username,
                action,
                target,
                detail,
            })
            .collect())
    }
}
