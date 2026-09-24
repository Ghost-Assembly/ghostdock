//! API tokens: issued once, stored as a hash, checked on every request.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use shared::token::Permission;

use crate::users::UserRow;
use crate::{Error, Result, Store};

/// Marks a string as a GhostDock token, for people and for secret scanners.
pub const SECRET_PREFIX: &str = "ghostdock_";
/// How much of the secret is kept in the clear to identify it.
const PREFIX_LEN: usize = SECRET_PREFIX.len() + 6;
/// `last_used_at` is written at most this often per token, so a busy client
/// does not turn every read into a write.
const TOUCH_INTERVAL_SECS: i64 = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRow {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub prefix: String,
    pub permissions: Vec<Permission>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub expires_at: Option<i64>,
}

type TokenTuple = (
    i64,
    i64,
    String,
    String,
    String,
    i64,
    Option<i64>,
    Option<i64>,
);

fn to_row(t: TokenTuple) -> TokenRow {
    let (id, user_id, name, prefix, permissions, created_at, last_used_at, expires_at) = t;
    TokenRow {
        id,
        user_id,
        name,
        prefix,
        permissions: parse_permissions(&permissions),
        created_at,
        last_used_at,
        expires_at,
    }
}

fn parse_permissions(stored: &str) -> Vec<Permission> {
    let mut list: Vec<_> = stored
        .split_whitespace()
        .filter_map(Permission::parse)
        .collect();
    list.sort_unstable();
    list.dedup();
    list
}

fn format_permissions(permissions: &[Permission]) -> String {
    let mut list = permissions.to_vec();
    list.sort_unstable();
    list.dedup();
    list.iter()
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn hash(secret: &str) -> Vec<u8> {
    Sha256::digest(secret.as_bytes()).to_vec()
}

fn generate() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| {
        Error::Secret(crate::secrets::SecretError::Io {
            context: "generating a token".to_owned(),
            source: std::io::Error::other(e.to_string()),
        })
    })?;
    Ok(format!("{SECRET_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes)))
}

impl Store {
    /// Issues a token. Returns the row and the secret, which is not stored
    /// and cannot be recovered afterwards.
    pub async fn token_create(
        &self,
        user_id: i64,
        name: &str,
        permissions: &[Permission],
        expires_at: Option<i64>,
    ) -> Result<(TokenRow, String)> {
        let secret = generate()?;
        let prefix = &secret[..PREFIX_LEN];
        let row = sqlx::query_as::<_, TokenTuple>(
            "INSERT INTO api_tokens
                 (user_id, name, prefix, secret_hash, permissions, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, unixepoch(), ?6)
             RETURNING id, user_id, name, prefix, permissions, created_at,
                       last_used_at, expires_at",
        )
        .bind(user_id)
        .bind(name)
        .bind(prefix)
        .bind(hash(&secret))
        .bind(format_permissions(permissions))
        .bind(expires_at)
        .fetch_one(self.pool())
        .await
        .map_err(|e| {
            if e.as_database_error()
                .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
            {
                Error::NameTaken
            } else {
                Error::Sqlx(e)
            }
        })?;
        Ok((to_row(row), secret))
    }

    /// An account's tokens, newest first.
    pub async fn tokens_for_user(&self, user_id: i64) -> Result<Vec<TokenRow>> {
        let rows = sqlx::query_as::<_, TokenTuple>(
            "SELECT id, user_id, name, prefix, permissions, created_at, last_used_at, expires_at
             FROM api_tokens WHERE user_id = ?1 ORDER BY id DESC",
        )
        .bind(user_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(to_row).collect())
    }

    /// Revokes one of an account's own tokens.
    pub async fn token_delete(&self, user_id: i64, id: i64) -> Result<()> {
        let done = sqlx::query("DELETE FROM api_tokens WHERE id = ?1 AND user_id = ?2")
            .bind(id)
            .bind(user_id)
            .execute(self.pool())
            .await?;
        if done.rows_affected() == 1 {
            Ok(())
        } else {
            Err(Error::NotFound)
        }
    }

    /// The token and account a presented secret belongs to, if it is known
    /// and unexpired. Records the use.
    pub async fn token_authenticate(&self, secret: &str) -> Result<Option<(TokenRow, UserRow)>> {
        let found = sqlx::query_as::<_, TokenTuple>(
            "SELECT id, user_id, name, prefix, permissions, created_at, last_used_at, expires_at
             FROM api_tokens
             WHERE secret_hash = ?1 AND (expires_at IS NULL OR expires_at > unixepoch())",
        )
        .bind(hash(secret))
        .fetch_optional(self.pool())
        .await?;
        let Some(token) = found.map(to_row) else {
            return Ok(None);
        };
        let Some(user) = self.user_by_id(token.user_id).await? else {
            return Ok(None);
        };

        sqlx::query(
            "UPDATE api_tokens SET last_used_at = unixepoch()
             WHERE id = ?1 AND (last_used_at IS NULL OR last_used_at <= unixepoch() - ?2)",
        )
        .bind(token.id)
        .bind(TOUCH_INTERVAL_SECS)
        .execute(self.pool())
        .await?;
        Ok(Some((token, user)))
    }
}
