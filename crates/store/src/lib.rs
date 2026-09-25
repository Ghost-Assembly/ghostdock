//! SQLite persistence and migrations.
//!
//! ARCHITECTURAL INVARIANT: holds no business rules. Secrets are
//! encrypted at rest here and are write-only across the API boundary —
//! they must never be returned in a response or written to a log.

pub mod audit;
pub mod hosts;
pub mod metrics;
pub mod secrets;
pub mod sessions;
pub mod sources;
pub mod stacks;
pub mod tokens;
pub mod updates;
pub mod users;

use sqlx::SqlitePool;

use crate::secrets::Cipher;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

/// Errors raised by the persistence layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("username is already taken")]
    UsernameTaken,
    #[error("a stack with that name already exists")]
    SlugTaken,
    #[error("no such record")]
    NotFound,
    #[error(transparent)]
    Secret(#[from] crate::secrets::SecretError),
    #[error("still in use by something else")]
    InUse,
    #[error("the last account cannot be removed")]
    LastAccount,
    #[error("that name is already in use")]
    NameTaken,
    #[error("an account already exists")]
    AccountsExist,
}

impl Error {
    /// Whether SQLite found the file damaged or not a database at all, as
    /// opposed to busy, locked, unreadable or out of space: the difference
    /// between a file worth setting aside and one worth waiting for.
    #[must_use]
    pub fn is_corrupt(&self) -> bool {
        /// SQLITE_CORRUPT and SQLITE_NOTADB.
        const DAMAGED: [i32; 2] = [11, 26];
        let cause = match self {
            Self::Sqlx(e)
            | Self::Migrate(
                sqlx::migrate::MigrateError::Execute(e)
                | sqlx::migrate::MigrateError::ExecuteMigration(e, _),
            ) => e,
            _ => return false,
        };
        cause
            .as_database_error()
            .and_then(|db| db.code())
            .and_then(|code| code.parse::<i32>().ok())
            // Extended codes carry the primary one in the low byte.
            .is_some_and(|code| DAMAGED.contains(&(code & 0xff)))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A stored Unix time as a date. A value out of range reads as the epoch
/// rather than failing the whole row.
#[must_use]
pub fn timestamp(secs: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(secs, 0).unwrap_or_default()
}

/// Maps a write's error to `taken` when it broke a UNIQUE constraint.
///
/// The index, not a prior SELECT, is the authority on duplicates: checking
/// first would race a concurrent write of the same name.
fn unique_or(taken: Error) -> impl FnOnce(sqlx::Error) -> Error {
    move |e| {
        if e.as_database_error()
            .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
        {
            taken
        } else {
            Error::Sqlx(e)
        }
    }
}

/// A handle to the GhostDock database.
///
/// Cloning is cheap: the underlying pool is shared. One pool serves the
/// whole process, including sessions — SQLite permits a single writer, so
/// a second pool against the same file would only create contention.
#[derive(Debug, Clone)]
pub struct Store {
    pool: SqlitePool,
    cipher: Cipher,
}

impl Store {
    /// Opens (creating if absent) the database at `path` and runs migrations.
    ///
    /// The cipher is supplied rather than derived here: key material is
    /// configuration, and this crate deliberately does not read it.
    pub async fn open(path: &str, cipher: Cipher) -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true);
        Self::from_options(opts, 5, cipher).await
    }

    /// Opens a private in-memory database. Intended for tests.
    ///
    /// Capped at a single permanent connection: each connection to
    /// `:memory:` gets its *own* private database, so a larger pool would
    /// scatter writes across several databases, and letting the pool reap an
    /// idle connection would discard the data entirely.
    /// Uses a fresh random key: a test database is discarded, and a fixed
    /// key here would be one keystroke away from becoming a default.
    pub async fn open_in_memory() -> Result<Self> {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).expect("system randomness");
        let opts = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true);
        Self::from_options(opts, 1, Cipher::new(&key)).await
    }

    async fn from_options(
        opts: SqliteConnectOptions,
        max_connections: u32,
        cipher: Cipher,
    ) -> Result<Self> {
        let mut pool = SqlitePoolOptions::new().max_connections(max_connections);
        if max_connections == 1 {
            pool = pool
                .min_connections(1)
                .idle_timeout(None)
                .max_lifetime(None);
        }
        let pool = pool.connect_with(opts).await?;
        let store = Self { pool, cipher };
        store.migrate().await?;
        Ok(store)
    }

    /// Applies any outstanding migrations.
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    /// Seals and opens stored secrets.
    #[must_use]
    pub fn cipher(&self) -> &Cipher {
        &self.cipher
    }

    /// The underlying pool, for callers that need it directly.
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
