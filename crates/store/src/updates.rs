//! What is waiting to be applied.

use chrono::DateTime;
use shared::update::{ImageStatus, UpdateStatus};

use crate::{Result, Store};

impl Store {
    /// Turns automatic application on or off for a stack.
    pub async fn stack_set_auto_apply(&self, id: i64, auto_apply: bool) -> Result<()> {
        sqlx::query("UPDATE stacks SET auto_apply = ?2 WHERE id = ?1")
            .bind(id)
            .bind(i64::from(auto_apply))
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn stack_auto_apply(&self, id: i64) -> Result<bool> {
        let row = sqlx::query_as::<_, (i64,)>("SELECT auto_apply FROM stacks WHERE id = ?1")
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
        Ok(row.is_some_and(|(value,)| value != 0))
    }

    /// Stacks that apply updates without being asked.
    pub async fn stacks_with_auto_apply(&self) -> Result<Vec<i64>> {
        let rows = sqlx::query_as::<_, (i64,)>("SELECT id FROM stacks WHERE auto_apply != 0")
            .fetch_all(self.pool())
            .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Records the outcome of a check, replacing the previous one.
    ///
    /// Wholesale, because a service removed from the compose file should stop
    /// being reported; merging would leave its image listed forever.
    pub async fn update_record(
        &self,
        stack_id: i64,
        remote_commit: Option<&str>,
        error: Option<&str>,
        images: &[ImageStatus],
    ) -> Result<()> {
        let mut tx = self.pool().begin().await?;

        sqlx::query(
            "INSERT INTO update_checks (stack_id, checked_at, remote_commit, error)
             VALUES (?1, unixepoch(), ?2, ?3)
             ON CONFLICT (stack_id) DO UPDATE SET
                 checked_at = unixepoch(),
                 remote_commit = excluded.remote_commit,
                 error = excluded.error",
        )
        .bind(stack_id)
        .bind(remote_commit)
        .bind(error)
        .execute(&mut *tx)
        .await?;

        sqlx::query("DELETE FROM image_checks WHERE stack_id = ?1")
            .bind(stack_id)
            .execute(&mut *tx)
            .await?;

        for image in images {
            sqlx::query(
                "INSERT INTO image_checks
                     (stack_id, image, running_digest, available_digest, error)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(stack_id)
            .bind(&image.image)
            .bind(image.running.as_deref())
            .bind(image.available.as_deref())
            .bind(image.error.as_deref())
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// What is waiting for one stack.
    ///
    /// Returns an empty status when the stack has never been checked, which
    /// reads as "nothing known yet" rather than "nothing waiting".
    pub async fn update_status(&self, stack_id: i64) -> Result<UpdateStatus> {
        let check = sqlx::query_as::<_, (i64, Option<String>, Option<String>)>(
            "SELECT checked_at, remote_commit, error FROM update_checks WHERE stack_id = ?1",
        )
        .bind(stack_id)
        .fetch_optional(self.pool())
        .await?;

        let deployed =
            sqlx::query_as::<_, (Option<String>,)>("SELECT last_commit FROM stacks WHERE id = ?1")
                .bind(stack_id)
                .fetch_optional(self.pool())
                .await?
                .and_then(|(commit,)| commit);

        let images = sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<String>)>(
            "SELECT image, running_digest, available_digest, error
             FROM image_checks WHERE stack_id = ?1 ORDER BY image",
        )
        .bind(stack_id)
        .fetch_all(self.pool())
        .await?
        .into_iter()
        .map(|(image, running, available, error)| ImageStatus {
            image,
            running,
            available,
            error,
        })
        .collect();

        let (checked_at, remote_commit, error) = match check {
            Some((at, commit, error)) => (DateTime::from_timestamp(at, 0), commit, error),
            None => (None, None, None),
        };

        Ok(UpdateStatus {
            checked_at,
            remote_commit,
            deployed_commit: deployed,
            images,
            error,
        })
    }
}
