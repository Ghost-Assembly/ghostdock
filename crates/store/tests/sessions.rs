//! Storage contract for sessions.
//!
//! These pin the behaviour the web-facing `SessionStore` adapter depends on,
//! notably that `session_create` reports id collisions instead of silently
//! overwriting an existing session.

use store::Store;
use store::sessions::SessionRow;

const NOW: i64 = 1_700_000_000;

fn row(id: &str, expiry: i64) -> SessionRow {
    SessionRow {
        id: id.to_owned(),
        data: b"payload".to_vec(),
        expiry_date: expiry,
    }
}

async fn store() -> Store {
    Store::open_in_memory().await.expect("in-memory store")
}

#[tokio::test]
async fn create_then_load_round_trips() {
    let s = store().await;
    let r = row("aaa", NOW + 60);
    assert!(s.session_create(&r).await.unwrap(), "first insert succeeds");

    let loaded = s.session_load("aaa", NOW).await.unwrap();
    assert_eq!(loaded, Some(r));
}

#[tokio::test]
async fn create_reports_id_collision_without_overwriting() {
    let s = store().await;
    let original = row("dup", NOW + 60);
    assert!(s.session_create(&original).await.unwrap());

    let mut intruder = row("dup", NOW + 60);
    intruder.data = b"intruder".to_vec();
    assert!(
        !s.session_create(&intruder).await.unwrap(),
        "colliding insert must report failure"
    );

    let loaded = s.session_load("dup", NOW).await.unwrap().unwrap();
    assert_eq!(loaded.data, b"payload".to_vec(), "original must survive");
}

#[tokio::test]
async fn save_overwrites_existing_session() {
    let s = store().await;
    s.session_save(&row("k", NOW + 60)).await.unwrap();

    let mut updated = row("k", NOW + 120);
    updated.data = b"updated".to_vec();
    s.session_save(&updated).await.unwrap();

    assert_eq!(s.session_load("k", NOW).await.unwrap(), Some(updated));
}

#[tokio::test]
async fn load_ignores_expired_and_unknown_sessions() {
    let s = store().await;
    s.session_save(&row("expired", NOW - 1)).await.unwrap();
    s.session_save(&row("boundary", NOW)).await.unwrap();
    s.session_save(&row("live", NOW + 1)).await.unwrap();

    assert_eq!(s.session_load("expired", NOW).await.unwrap(), None);
    assert_eq!(
        s.session_load("boundary", NOW).await.unwrap(),
        None,
        "a session expiring exactly now is expired"
    );
    assert!(s.session_load("live", NOW).await.unwrap().is_some());
    assert_eq!(s.session_load("nope", NOW).await.unwrap(), None);
}

#[tokio::test]
async fn delete_removes_session_and_tolerates_absence() {
    let s = store().await;
    s.session_save(&row("gone", NOW + 60)).await.unwrap();

    s.session_delete("gone").await.unwrap();
    assert_eq!(s.session_load("gone", NOW).await.unwrap(), None);

    s.session_delete("gone")
        .await
        .expect("deleting an absent session is not an error");
}

#[tokio::test]
async fn delete_expired_removes_only_expired_sessions() {
    let s = store().await;
    s.session_save(&row("old1", NOW - 10)).await.unwrap();
    s.session_save(&row("old2", NOW)).await.unwrap();
    s.session_save(&row("keep", NOW + 10)).await.unwrap();

    assert_eq!(s.session_delete_expired(NOW).await.unwrap(), 2);
    assert!(s.session_load("keep", NOW).await.unwrap().is_some());
    assert_eq!(s.session_delete_expired(NOW).await.unwrap(), 0);
}
