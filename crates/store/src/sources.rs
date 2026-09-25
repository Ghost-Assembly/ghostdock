//! Git remotes, credentials, and stack environment variables.
//!
//! Every secret written here is sealed and bound to a purpose, and no query
//! in this module returns one to a caller that only wants to display it.

use shared::source::{Credential, Repo};

use crate::secrets::purpose;
use crate::{Error, Result, Store, timestamp, unique_or};

impl Store {
    // ---- credentials --------------------------------------------------

    /// Stores a credential, sealing the secret.
    pub async fn credential_create(
        &self,
        name: &str,
        username: &str,
        secret: &str,
    ) -> Result<Credential> {
        let sealed = self.cipher().seal(purpose::GIT_CREDENTIAL, secret)?;

        let (id, created_at) = sqlx::query_as::<_, (i64, i64)>(
            "INSERT INTO credentials (name, username, secret_sealed, created_at)
             VALUES (?1, ?2, ?3, unixepoch())
             RETURNING id, created_at",
        )
        .bind(name)
        .bind(username)
        .bind(&sealed)
        .fetch_one(self.pool())
        .await
        .map_err(unique_or(Error::SlugTaken))?;

        Ok(Credential {
            id,
            name: name.to_owned(),
            username: username.to_owned(),
            created_at: timestamp(created_at),
        })
    }

    /// All credentials, without their secrets.
    pub async fn credentials_list(&self) -> Result<Vec<Credential>> {
        let rows = sqlx::query_as::<_, (i64, String, String, i64)>(
            "SELECT id, name, username, created_at FROM credentials ORDER BY name",
        )
        .fetch_all(self.pool())
        .await?;

        Ok(rows
            .into_iter()
            .map(|(id, name, username, created_at)| Credential {
                id,
                name,
                username,
                created_at: timestamp(created_at),
            })
            .collect())
    }

    /// The username and secret, for handing to git.
    ///
    /// The only path that opens a credential. Callers use it and drop it;
    /// nothing serialises the result.
    pub async fn credential_secret(&self, id: i64) -> Result<Option<(String, String)>> {
        let row = sqlx::query_as::<_, (String, String)>(
            "SELECT username, secret_sealed FROM credentials WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;

        match row {
            Some((username, sealed)) => {
                let secret = self.cipher().open(purpose::GIT_CREDENTIAL, &sealed)?;
                Ok(Some((username, secret)))
            }
            None => Ok(None),
        }
    }

    pub async fn credential_delete(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM credentials WHERE id = ?1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    // ---- repositories -------------------------------------------------

    pub async fn repo_create(&self, url: &str, credential_id: Option<i64>) -> Result<Repo> {
        let (id,) = sqlx::query_as::<_, (i64,)>(
            "INSERT INTO repos (url, credential_id, created_at)
             VALUES (?1, ?2, unixepoch())
             RETURNING id",
        )
        .bind(url)
        .bind(credential_id)
        .fetch_one(self.pool())
        .await
        .map_err(unique_or(Error::SlugTaken))?;

        self.repo_by_id(id).await?.ok_or(Error::NotFound)
    }

    pub async fn repos_list(&self) -> Result<Vec<Repo>> {
        let rows = sqlx::query_as::<_, (i64, String, Option<i64>, Option<String>)>(
            "SELECT r.id, r.url, r.credential_id, c.name
             FROM repos r LEFT JOIN credentials c ON c.id = r.credential_id
             ORDER BY r.url",
        )
        .fetch_all(self.pool())
        .await?;

        Ok(rows.into_iter().map(to_repo).collect())
    }

    /// Replaces (or with `None`, removes) the credential a repository uses.
    pub async fn repo_set_credential(&self, id: i64, credential_id: Option<i64>) -> Result<Repo> {
        let done = sqlx::query("UPDATE repos SET credential_id = ?2 WHERE id = ?1")
            .bind(id)
            .bind(credential_id)
            .execute(self.pool())
            .await?;
        if done.rows_affected() == 0 {
            return Err(Error::NotFound);
        }
        self.repo_by_id(id).await?.ok_or(Error::NotFound)
    }

    pub async fn repo_by_id(&self, id: i64) -> Result<Option<Repo>> {
        let row = sqlx::query_as::<_, (i64, String, Option<i64>, Option<String>)>(
            "SELECT r.id, r.url, r.credential_id, c.name
             FROM repos r LEFT JOIN credentials c ON c.id = r.credential_id
             WHERE r.id = ?1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;

        Ok(row.map(to_repo))
    }

    /// Removes a repository.
    ///
    /// Refused while a stack still points at it: the schema uses RESTRICT
    /// rather than CASCADE, because deleting a repository should not
    /// silently delete the stacks defined in it.
    pub async fn repo_delete(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM repos WHERE id = ?1")
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(|e| {
                if e.as_database_error()
                    .is_some_and(sqlx::error::DatabaseError::is_foreign_key_violation)
                {
                    Error::InUse
                } else {
                    Error::Sqlx(e)
                }
            })?;
        Ok(())
    }

    // ---- stack environment --------------------------------------------

    /// Replaces a stack's environment wholesale.
    ///
    /// Wholesale rather than per-key: the caller is editing a set, and a
    /// partial update would leave variables the user deleted still applied.
    pub async fn stack_env_set(&self, stack_id: i64, vars: &[(String, String)]) -> Result<()> {
        let mut tx = self.pool().begin().await?;

        sqlx::query("DELETE FROM stack_env WHERE stack_id = ?1")
            .bind(stack_id)
            .execute(&mut *tx)
            .await?;

        for (key, value) in vars {
            let sealed = self.cipher().seal(purpose::STACK_ENV, value)?;
            sqlx::query("INSERT INTO stack_env (stack_id, key, value_sealed) VALUES (?1, ?2, ?3)")
                .bind(stack_id)
                .bind(key)
                .bind(&sealed)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Sets or replaces one variable.
    ///
    /// Per-key as well as wholesale, because values are write-only: with only
    /// a wholesale replace, changing one variable would mean retyping every
    /// secret the stack has.
    pub async fn stack_env_set_one(&self, stack_id: i64, key: &str, value: &str) -> Result<()> {
        let sealed = self.cipher().seal(purpose::STACK_ENV, value)?;
        sqlx::query(
            "INSERT INTO stack_env (stack_id, key, value_sealed) VALUES (?1, ?2, ?3)
             ON CONFLICT (stack_id, key) DO UPDATE SET value_sealed = excluded.value_sealed",
        )
        .bind(stack_id)
        .bind(key)
        .bind(&sealed)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Removes one variable. Removing an absent one is not an error.
    pub async fn stack_env_delete_one(&self, stack_id: i64, key: &str) -> Result<()> {
        sqlx::query("DELETE FROM stack_env WHERE stack_id = ?1 AND key = ?2")
            .bind(stack_id)
            .bind(key)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// The stack's variables, decrypted, for writing a `.env`.
    pub async fn stack_env_get(&self, stack_id: i64) -> Result<Vec<(String, String)>> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT key, value_sealed FROM stack_env WHERE stack_id = ?1 ORDER BY key",
        )
        .bind(stack_id)
        .fetch_all(self.pool())
        .await?;

        rows.into_iter()
            .map(|(key, sealed)| {
                let value = self.cipher().open(purpose::STACK_ENV, &sealed)?;
                Ok((key, value))
            })
            .collect()
    }

    /// Variable names alone, for showing what a stack defines.
    pub async fn stack_env_keys(&self, stack_id: i64) -> Result<Vec<String>> {
        let rows = sqlx::query_as::<_, (String,)>(
            "SELECT key FROM stack_env WHERE stack_id = ?1 ORDER BY key",
        )
        .bind(stack_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(|(key,)| key).collect())
    }
}

fn to_repo(
    (id, url, credential_id, credential_name): (i64, String, Option<i64>, Option<String>),
) -> Repo {
    Repo {
        id,
        url,
        credential_id,
        credential_name,
    }
}
