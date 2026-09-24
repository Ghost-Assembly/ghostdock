//! Docker hosts.

use crate::{Result, Store};
use shared::host::Host;

/// Id of the single host seeded by the initial migration.
pub const LOCAL_HOST_ID: i64 = 1;

impl Store {
    /// All known hosts, ordered by id.
    pub async fn hosts_list(&self) -> Result<Vec<Host>> {
        let rows = sqlx::query_as::<_, (i64, String)>("SELECT id, name FROM hosts ORDER BY id")
            .fetch_all(self.pool())
            .await?;
        Ok(rows
            .into_iter()
            .map(|(id, name)| Host { id, name })
            .collect())
    }

    /// A single host by id.
    pub async fn host_by_id(&self, id: i64) -> Result<Option<Host>> {
        let found = sqlx::query_as::<_, (i64, String)>("SELECT id, name FROM hosts WHERE id = ?1")
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
        Ok(found.map(|(id, name)| Host { id, name }))
    }
}
