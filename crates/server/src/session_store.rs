//! `tower-sessions` adapter over GhostDock's own session table.
//!
//! Owned rather than imported. The published sqlx-backed store is built
//! against an older `tower-sessions-core`, so depending on it pins both
//! `tower-sessions` and `sqlx` to older releases across the whole workspace.
//! The contract is four small methods, so this is the cheaper side of the
//! trade — and it lets sessions share the single SQLite pool, which matters
//! because SQLite permits exactly one writer.
//!
//! This module is the only place that knows both the web session types and
//! the storage layer; `store` itself stays HTTP-agnostic.

use async_trait::async_trait;
use store::Store;
use store::sessions::SessionRow;
use tower_sessions::cookie::time::OffsetDateTime;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, ExpiredDeletion, SessionStore};

/// How many fresh ids to try before giving up on a collision.
///
/// Ids are 128-bit random values, so a single collision is already
/// vanishingly unlikely; repeated ones mean something is wrong and we should
/// surface an error rather than spin.
const MAX_ID_ATTEMPTS: u8 = 8;

/// A `tower-sessions` store backed by the GhostDock database.
#[derive(Debug, Clone)]
pub struct SqliteSessionStore {
    store: Store,
}

impl SqliteSessionStore {
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

fn backend<E: std::fmt::Display>(e: E) -> session_store::Error {
    session_store::Error::Backend(e.to_string())
}

/// Seconds since the epoch, rounded **up**.
///
/// Truncating would expire a session up to a second early; erring toward a
/// slightly longer session is the safer direction for a login.
fn expiry_seconds(at: OffsetDateTime) -> i64 {
    let secs = at.unix_timestamp();
    if at.nanosecond() > 0 { secs + 1 } else { secs }
}

fn encode(record: &Record) -> session_store::Result<SessionRow> {
    Ok(SessionRow {
        id: record.id.to_string(),
        data: serde_json::to_vec(&record.data)
            .map_err(|e| session_store::Error::Encode(e.to_string()))?,
        expiry_date: expiry_seconds(record.expiry_date),
    })
}

fn decode(row: SessionRow) -> session_store::Result<Record> {
    let id = row
        .id
        .parse::<Id>()
        .map_err(|e| session_store::Error::Decode(format!("invalid session id: {e}")))?;
    let data = serde_json::from_slice(&row.data)
        .map_err(|e| session_store::Error::Decode(e.to_string()))?;
    let expiry_date = OffsetDateTime::from_unix_timestamp(row.expiry_date)
        .map_err(|e| session_store::Error::Decode(e.to_string()))?;
    Ok(Record {
        id,
        data,
        expiry_date,
    })
}

fn now_seconds() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    /// Inserts the record, assigning a fresh id if the chosen one is taken.
    ///
    /// The default implementation delegates to `save`, which would silently
    /// overwrite an existing session on collision. That is a session-fixation
    /// hazard, so we insert conditionally and retry instead.
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        for _ in 0..MAX_ID_ATTEMPTS {
            let row = encode(record)?;
            if self.store.session_create(&row).await.map_err(backend)? {
                return Ok(());
            }
            record.id = Id::default();
        }
        Err(session_store::Error::Backend(
            "could not allocate an unused session id".to_owned(),
        ))
    }

    async fn save(&self, record: &Record) -> session_store::Result<()> {
        let row = encode(record)?;
        self.store.session_save(&row).await.map_err(backend)
    }

    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        let found = self
            .store
            .session_load(&session_id.to_string(), now_seconds())
            .await
            .map_err(backend)?;
        found.map(decode).transpose()
    }

    async fn delete(&self, session_id: &Id) -> session_store::Result<()> {
        self.store
            .session_delete(&session_id.to_string())
            .await
            .map_err(backend)
    }
}

#[async_trait]
impl ExpiredDeletion for SqliteSessionStore {
    async fn delete_expired(&self) -> session_store::Result<()> {
        let removed = self
            .store
            .session_delete_expired(now_seconds())
            .await
            .map_err(backend)?;
        if removed > 0 {
            tracing::debug!(removed, "swept expired sessions");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_sessions::cookie::time::Duration;

    async fn store() -> SqliteSessionStore {
        SqliteSessionStore::new(Store::open_in_memory().await.expect("in-memory store"))
    }

    fn record(expires_in: Duration) -> Record {
        let mut data = std::collections::HashMap::new();
        data.insert("user_id".to_owned(), serde_json::json!(42));
        Record {
            id: Id::default(),
            data,
            expiry_date: OffsetDateTime::now_utc() + expires_in,
        }
    }

    #[tokio::test]
    async fn create_then_load_round_trips_data() {
        let s = store().await;
        let mut r = record(Duration::minutes(5));
        s.create(&mut r).await.unwrap();

        let loaded = s.load(&r.id).await.unwrap().expect("session present");
        assert_eq!(loaded.id, r.id);
        assert_eq!(loaded.data, r.data);
    }

    #[tokio::test]
    async fn create_assigns_a_new_id_rather_than_overwriting_on_collision() {
        let s = store().await;
        let mut first = record(Duration::minutes(5));
        s.create(&mut first).await.unwrap();

        // Force a collision by reusing the id that is already taken.
        let mut second = record(Duration::minutes(5));
        second.id = first.id;
        second
            .data
            .insert("user_id".to_owned(), serde_json::json!(99));
        s.create(&mut second).await.unwrap();

        assert_ne!(second.id, first.id, "colliding create must take a new id");

        let original = s.load(&first.id).await.unwrap().expect("original present");
        assert_eq!(
            original.data.get("user_id"),
            Some(&serde_json::json!(42)),
            "the original session must not be overwritten"
        );
        assert!(s.load(&second.id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn load_returns_none_for_expired_and_unknown() {
        let s = store().await;
        let mut expired = record(Duration::seconds(-30));
        s.create(&mut expired).await.unwrap();
        assert!(s.load(&expired.id).await.unwrap().is_none());
        assert!(s.load(&Id::default()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn save_updates_and_delete_removes() {
        let s = store().await;
        let mut r = record(Duration::minutes(5));
        s.create(&mut r).await.unwrap();

        r.data.insert("theme".to_owned(), serde_json::json!("dark"));
        s.save(&r).await.unwrap();
        let loaded = s.load(&r.id).await.unwrap().unwrap();
        assert_eq!(loaded.data.get("theme"), Some(&serde_json::json!("dark")));

        s.delete(&r.id).await.unwrap();
        assert!(s.load(&r.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_expired_sweeps_only_expired_sessions() {
        let s = store().await;
        let mut old = record(Duration::seconds(-30));
        let mut live = record(Duration::minutes(5));
        s.create(&mut old).await.unwrap();
        s.create(&mut live).await.unwrap();

        s.delete_expired().await.unwrap();

        // The live session survives; the expired one is gone from storage,
        // not merely filtered out on read.
        assert!(s.load(&live.id).await.unwrap().is_some());
        assert!(
            s.store
                .session_load(&old.id.to_string(), i64::MIN)
                .await
                .unwrap()
                .is_none(),
            "expired row must be deleted, not just hidden"
        );
    }

    /// Regression guard. The published sqlx-backed store is built against an
    /// older `tower-sessions-core`, so wiring it into `SessionManagerLayer`
    /// fails with "multiple different versions of crate tower_sessions_core".
    /// Owning the store is what lets this compile against current releases;
    /// if a future dependency bump reintroduces the split, this stops building.
    #[tokio::test]
    async fn integrates_with_the_session_manager_layer() {
        let s = store().await;
        let _layer = tower_sessions::SessionManagerLayer::new(s);
    }

    #[tokio::test]
    async fn expiry_rounds_up_so_sessions_never_end_early() {
        let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        assert_eq!(expiry_seconds(base), 1_700_000_000);
        assert_eq!(
            expiry_seconds(base + Duration::milliseconds(1)),
            1_700_000_001
        );
    }
}
