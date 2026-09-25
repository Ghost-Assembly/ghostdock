//! User accounts.

use crate::{Error, Result, Store, unique_or};

/// A user as stored, including the password hash.
///
/// Never serialise this. The API exposes [`shared::auth::User`], which has
/// no password material on it at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub created_at: i64,
    /// See migration 0007: sessions issued under an older value are void.
    pub session_epoch: i64,
}

impl UserRow {
    /// Projects to the public wire type.
    #[must_use]
    pub fn to_public(&self) -> shared::auth::User {
        shared::auth::User {
            id: self.id,
            username: self.username.clone(),
        }
    }
}

type UserTuple = (i64, String, String, i64, i64);

fn to_row((id, username, password_hash, created_at, session_epoch): UserTuple) -> UserRow {
    UserRow {
        id,
        username,
        password_hash,
        created_at,
        session_epoch,
    }
}

impl Store {
    /// Number of accounts. Zero means the instance is not yet bootstrapped.
    pub async fn user_count(&self) -> Result<i64> {
        let (count,) = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM users")
            .fetch_one(self.pool())
            .await?;
        Ok(count)
    }

    /// Creates a user, failing with [`Error::UsernameTaken`] on a duplicate.
    pub async fn user_create(&self, username: &str, password_hash: &str) -> Result<UserRow> {
        let row = sqlx::query_as::<_, UserTuple>(
            "INSERT INTO users (username, password_hash, created_at)
             VALUES (?1, ?2, unixepoch())
             RETURNING id, username, password_hash, created_at, session_epoch",
        )
        .bind(username)
        .bind(password_hash)
        .fetch_one(self.pool())
        .await
        .map_err(unique_or(Error::UsernameTaken))?;
        Ok(to_row(row))
    }

    /// Creates the first account, failing with [`Error::AccountsExist`] if
    /// there is any account already.
    ///
    /// One statement, so "is there anyone yet" and "add this one" cannot be
    /// split by another request doing the same: two people setting up at
    /// once cannot both become the first administrator.
    pub async fn user_create_first(&self, username: &str, password_hash: &str) -> Result<UserRow> {
        let row = sqlx::query_as::<_, UserTuple>(
            "INSERT INTO users (username, password_hash, created_at)
             SELECT ?1, ?2, unixepoch()
             WHERE NOT EXISTS (SELECT 1 FROM users)
             RETURNING id, username, password_hash, created_at, session_epoch",
        )
        .bind(username)
        .bind(password_hash)
        .fetch_optional(self.pool())
        .await?;
        row.map(to_row).ok_or(Error::AccountsExist)
    }

    /// Looks a user up by name, case-insensitively.
    pub async fn user_by_username(&self, username: &str) -> Result<Option<UserRow>> {
        let found = sqlx::query_as::<_, UserTuple>(
            "SELECT id, username, password_hash, created_at, session_epoch
             FROM users WHERE username = ?1",
        )
        .bind(username)
        .fetch_optional(self.pool())
        .await?;
        Ok(found.map(to_row))
    }

    /// Looks a user up by id.
    pub async fn user_by_id(&self, id: i64) -> Result<Option<UserRow>> {
        let found = sqlx::query_as::<_, UserTuple>(
            "SELECT id, username, password_hash, created_at, session_epoch
             FROM users WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        Ok(found.map(to_row))
    }

    /// Every account, oldest first.
    pub async fn users_list(&self) -> Result<Vec<UserRow>> {
        let rows = sqlx::query_as::<_, UserTuple>(
            "SELECT id, username, password_hash, created_at, session_epoch
             FROM users ORDER BY id",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(to_row).collect())
    }

    /// Removes an account, refusing to remove the last one.
    ///
    /// The count is checked inside the same statement as the delete. Two
    /// people removing each other at once would otherwise each see a second
    /// account, both succeed, and leave an instance that anyone can claim.
    pub async fn user_delete(&self, id: i64) -> Result<()> {
        let done =
            sqlx::query("DELETE FROM users WHERE id = ?1 AND (SELECT COUNT(*) FROM users) > 1")
                .bind(id)
                .execute(self.pool())
                .await?;
        if done.rows_affected() == 1 {
            return Ok(());
        }
        match self.user_by_id(id).await? {
            Some(_) => Err(Error::LastAccount),
            None => Err(Error::NotFound),
        }
    }

    /// Replaces a password hash and voids every session issued before now.
    /// Returns the new epoch, for the caller's own session to carry forward.
    pub async fn user_set_password(&self, id: i64, password_hash: &str) -> Result<i64> {
        let epoch = sqlx::query_as::<_, (i64,)>(
            "UPDATE users SET password_hash = ?2, session_epoch = session_epoch + 1
             WHERE id = ?1 RETURNING session_epoch",
        )
        .bind(id)
        .bind(password_hash)
        .fetch_optional(self.pool())
        .await?
        .ok_or(Error::NotFound)?;
        Ok(epoch.0)
    }
}
