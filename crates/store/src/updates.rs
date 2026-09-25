//! What is waiting to be applied.

use std::collections::HashMap;

use chrono::DateTime;
use shared::update::{ImageStatus, UpdateStatus};

use crate::{Result, Store};

/// Stacks with their last check, if any, filtered by `$filter`: rows for
/// [`CheckTuple`].
macro_rules! check_select {
    ($filter:literal) => {
        concat!(
            "SELECT s.id, s.auto_apply, s.last_commit, c.checked_at, c.remote_commit, c.error
             FROM stacks s LEFT JOIN update_checks c ON c.stack_id = s.id ",
            $filter
        )
    };
}

/// Image checks, filtered by `$filter`, in the order a status lists them:
/// rows for [`ImageTuple`].
macro_rules! image_select {
    ($filter:literal) => {
        concat!(
            "SELECT i.stack_id, i.image, i.running_digest, i.available_digest, i.error
             FROM image_checks i ",
            $filter,
            " ORDER BY i.stack_id, i.image"
        )
    };
}

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
        let check = sqlx::query_as::<_, CheckTuple>(check_select!("WHERE s.id = ?1"))
            .bind(stack_id)
            .fetch_optional(self.pool())
            .await?;
        let images = sqlx::query_as::<_, ImageTuple>(image_select!("WHERE i.stack_id = ?1"))
            .bind(stack_id)
            .fetch_all(self.pool())
            .await?;
        Ok(match check {
            Some(check) => to_status(check, images).0,
            None => UpdateStatus {
                images: images.into_iter().map(to_image).collect(),
                ..UpdateStatus::default()
            },
        })
    }

    /// [`Self::update_status`] for every stack on a host, with whether each
    /// applies updates by itself, by stack id. Two queries however many
    /// stacks there are.
    pub async fn update_statuses(
        &self,
        host_id: i64,
    ) -> Result<HashMap<i64, (UpdateStatus, bool)>> {
        let checks = sqlx::query_as::<_, CheckTuple>(check_select!("WHERE s.host_id = ?1"))
            .bind(host_id)
            .fetch_all(self.pool())
            .await?;
        let mut images: HashMap<i64, Vec<ImageTuple>> = HashMap::new();
        for image in sqlx::query_as::<_, ImageTuple>(image_select!(
            "JOIN stacks s ON s.id = i.stack_id WHERE s.host_id = ?1"
        ))
        .bind(host_id)
        .fetch_all(self.pool())
        .await?
        {
            images.entry(image.0).or_default().push(image);
        }
        Ok(checks
            .into_iter()
            .map(|check| {
                let id = check.0;
                (id, to_status(check, images.remove(&id).unwrap_or_default()))
            })
            .collect())
    }
}

/// id, auto_apply, last_commit, checked_at, remote_commit, error
type CheckTuple = (
    i64,
    i64,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
);

/// stack_id, image, running_digest, available_digest, error
type ImageTuple = (i64, String, Option<String>, Option<String>, Option<String>);

fn to_image((_, image, running, available, error): ImageTuple) -> ImageStatus {
    ImageStatus {
        image,
        running,
        available,
        error,
    }
}

/// A status and whether its stack applies updates by itself.
fn to_status(
    (_, auto_apply, deployed, checked_at, remote_commit, error): CheckTuple,
    images: Vec<ImageTuple>,
) -> (UpdateStatus, bool) {
    let status = UpdateStatus {
        checked_at: checked_at.and_then(|at| DateTime::from_timestamp(at, 0)),
        remote_commit,
        deployed_commit: deployed,
        images: images.into_iter().map(to_image).collect(),
        error,
    };
    (status, auto_apply != 0)
}
