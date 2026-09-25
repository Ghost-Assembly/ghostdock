//! User and host storage.

use store::Store;
use store::hosts::LOCAL_HOST_ID;

async fn store() -> Store {
    Store::open_in_memory().await.expect("in-memory store")
}

#[tokio::test]
async fn a_fresh_database_has_no_users() {
    assert_eq!(store().await.user_count().await.unwrap(), 0);
}

#[tokio::test]
async fn create_then_look_up_by_name_and_id() {
    let s = store().await;
    let created = s.user_create("admin", "$argon2id$fake").await.unwrap();

    assert_eq!(created.username, "admin");
    assert_eq!(s.user_count().await.unwrap(), 1);
    assert_eq!(
        s.user_by_username("admin").await.unwrap(),
        Some(created.clone())
    );
    assert_eq!(s.user_by_id(created.id).await.unwrap(), Some(created));
}

#[tokio::test]
async fn only_the_very_first_account_can_be_created_as_the_first() {
    // Checking for accounts and then inserting would let two people setting
    // up at once both become administrator. One statement cannot.
    let s = store().await;
    let first = s
        .user_create_first("admin", "$argon2id$fake")
        .await
        .unwrap();
    assert_eq!(first.username, "admin");

    assert!(matches!(
        s.user_create_first("second", "$argon2id$fake").await,
        Err(store::Error::AccountsExist)
    ));
    assert_eq!(s.user_count().await.unwrap(), 1);
}

#[tokio::test]
async fn concurrent_first_accounts_yield_exactly_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("race.db");
    let mut key = [0u8; 32];
    key[0] = 1;
    let s = Store::open(path.to_str().unwrap(), store::secrets::Cipher::new(&key))
        .await
        .unwrap();

    let attempts: Vec<_> = (0..8)
        .map(|i| {
            let s = s.clone();
            tokio::spawn(async move {
                s.user_create_first(&format!("admin{i}"), "$argon2id$fake")
                    .await
            })
        })
        .collect();
    let mut created = 0;
    for attempt in attempts {
        match attempt.await.unwrap() {
            Ok(_) => created += 1,
            Err(store::Error::AccountsExist) => {}
            Err(other) => panic!("{other}"),
        }
    }
    assert_eq!(created, 1);
    assert_eq!(s.user_count().await.unwrap(), 1);
}

#[tokio::test]
async fn lookup_is_case_insensitive() {
    let s = store().await;
    s.user_create("Admin", "$argon2id$fake").await.unwrap();

    assert!(
        s.user_by_username("admin").await.unwrap().is_some(),
        "a username typed in the wrong case must still find the account"
    );
    assert!(s.user_by_username("ADMIN").await.unwrap().is_some());
}

#[tokio::test]
async fn duplicate_usernames_are_rejected_regardless_of_case() {
    let s = store().await;
    s.user_create("admin", "$argon2id$fake").await.unwrap();

    assert!(matches!(
        s.user_create("admin", "$argon2id$other").await,
        Err(store::Error::UsernameTaken)
    ));
    assert!(
        matches!(
            s.user_create("ADMIN", "$argon2id$other").await,
            Err(store::Error::UsernameTaken)
        ),
        "case must not be a way to register a second account with one name"
    );
    assert_eq!(s.user_count().await.unwrap(), 1);
}

#[tokio::test]
async fn unknown_users_are_none_not_errors() {
    let s = store().await;
    assert_eq!(s.user_by_username("nobody").await.unwrap(), None);
    assert_eq!(s.user_by_id(999).await.unwrap(), None);
}

#[tokio::test]
async fn the_local_host_is_seeded_by_migration() {
    let s = store().await;
    let hosts = s.hosts_list().await.unwrap();

    assert_eq!(hosts.len(), 1, "exactly one host in v1");
    assert_eq!(hosts[0].id, LOCAL_HOST_ID);
    assert_eq!(hosts[0].name, "local");
    assert_eq!(
        s.host_by_id(LOCAL_HOST_ID).await.unwrap(),
        Some(hosts[0].clone())
    );
    assert_eq!(s.host_by_id(42).await.unwrap(), None);
}

#[tokio::test]
async fn accounts_are_listed_oldest_first() {
    let s = store().await;
    s.user_create("first", "h").await.unwrap();
    s.user_create("second", "h").await.unwrap();

    let names: Vec<_> = s
        .users_list()
        .await
        .unwrap()
        .into_iter()
        .map(|u| u.username)
        .collect();
    assert_eq!(names, ["first", "second"]);
}

#[tokio::test]
async fn an_account_can_be_removed_while_another_remains() {
    let s = store().await;
    s.user_create("keep", "h").await.unwrap();
    let gone = s.user_create("gone", "h").await.unwrap();

    s.user_delete(gone.id).await.unwrap();
    assert!(s.user_by_id(gone.id).await.unwrap().is_none());
    assert_eq!(s.user_count().await.unwrap(), 1);
}

#[tokio::test]
async fn the_last_account_cannot_be_removed() {
    // With no accounts the instance is back to first-run setup, which anyone
    // who can reach it may complete. The store refuses rather than trusting
    // every caller to have checked first.
    let s = store().await;
    let only = s.user_create("only", "h").await.unwrap();

    let err = s.user_delete(only.id).await.unwrap_err();
    assert!(matches!(err, store::Error::LastAccount), "{err:?}");
    assert_eq!(s.user_count().await.unwrap(), 1);
}

#[tokio::test]
async fn removing_an_unknown_account_is_not_found() {
    let s = store().await;
    s.user_create("a", "h").await.unwrap();
    s.user_create("b", "h").await.unwrap();
    let err = s.user_delete(999).await.unwrap_err();
    assert!(matches!(err, store::Error::NotFound), "{err:?}");
}

#[tokio::test]
async fn changing_a_password_moves_the_session_epoch_on() {
    let s = store().await;
    let u = s.user_create("a", "old-hash").await.unwrap();
    assert_eq!(u.session_epoch, 0);

    let epoch = s.user_set_password(u.id, "new-hash").await.unwrap();
    assert_eq!(epoch, 1);

    let after = s.user_by_id(u.id).await.unwrap().unwrap();
    assert_eq!(after.password_hash, "new-hash");
    assert_eq!(after.session_epoch, 1);
}
