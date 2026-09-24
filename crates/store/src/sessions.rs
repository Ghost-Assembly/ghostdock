//! Session persistence.
//!
//! Owned rather than imported: the published sqlx-backed session store is
//! built against an older `tower-sessions-core`, which would force both an
//! older `tower-sessions` and an older `sqlx` on the whole workspace. The
//! storage contract is four small queries, so owning it is cheaper than
//! carrying that constraint. The web-facing `SessionStore` adapter lives in
//! the server crate; this module knows nothing about HTTP.

use crate::{Result, Store};

/// A stored session: opaque bytes plus an absolute expiry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    /// Canonical textual session id.
    pub id: String,
    /// Serialised session payload. Opaque to this layer.
    pub data: Vec<u8>,
    /// Expiry as a Unix timestamp in seconds.
    pub expiry_date: i64,
}

impl Store {
    /// Inserts a session, failing if the id is already taken.
    ///
    /// Returns `false` on collision so the caller can retry with a fresh id
    /// rather than silently overwriting somebody else's session.
    pub async fn session_create(&self, row: &SessionRow) -> Result<bool> {
        let result = sqlx::query(
            "INSERT OR IGNORE INTO sessions (id, data, expiry_date) VALUES (?1, ?2, ?3)",
        )
        .bind(&row.id)
        .bind(&row.data)
        .bind(row.expiry_date)
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Inserts or replaces a session.
    pub async fn session_save(&self, row: &SessionRow) -> Result<()> {
        sqlx::query(
            "INSERT INTO sessions (id, data, expiry_date) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET data = excluded.data,
                                           expiry_date = excluded.expiry_date",
        )
        .bind(&row.id)
        .bind(&row.data)
        .bind(row.expiry_date)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Loads a session that has not expired as of `now`.
    pub async fn session_load(&self, id: &str, now: i64) -> Result<Option<SessionRow>> {
        // Strictly greater than: a session expiring exactly at `now` is spent.
        let found = sqlx::query_as::<_, (String, Vec<u8>, i64)>(
            "SELECT id, data, expiry_date FROM sessions WHERE id = ?1 AND expiry_date > ?2",
        )
        .bind(id)
        .bind(now)
        .fetch_optional(self.pool())
        .await?;

        Ok(found.map(|(id, data, expiry_date)| SessionRow {
            id,
            data,
            expiry_date,
        }))
    }

    /// Deletes a session. Deleting an absent session is not an error.
    pub async fn session_delete(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Deletes every session that expired at or before `now`, returning the count.
    pub async fn session_delete_expired(&self, now: i64) -> Result<u64> {
        let result = sqlx::query("DELETE FROM sessions WHERE expiry_date <= ?1")
            .bind(now)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected())
    }
}
