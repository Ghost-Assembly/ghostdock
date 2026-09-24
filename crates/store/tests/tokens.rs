//! API token storage.

use shared::token::Permission;
use store::Store;

async fn store_with_user() -> (Store, i64) {
    let s = Store::open_in_memory().await.expect("store");
    let u = s.user_create("admin", "h").await.unwrap();
    (s, u.id)
}

#[tokio::test]
async fn a_token_authenticates_as_its_owner_with_its_permissions() {
    let (s, user) = store_with_user().await;
    let (row, secret) = s
        .token_create(
            user,
            "ci",
            &[Permission::HostView, Permission::StacksDeploy],
            None,
        )
        .await
        .unwrap();

    assert!(
        secret.starts_with("ghostdock_"),
        "recognisable in a config file"
    );
    assert!(secret.starts_with(&row.prefix));

    let (found, owner) = s.token_authenticate(&secret).await.unwrap().expect("valid");
    assert_eq!(found.id, row.id);
    assert_eq!(owner.id, user);
    assert_eq!(
        found.permissions,
        [Permission::HostView, Permission::StacksDeploy]
    );
}

#[tokio::test]
async fn only_a_hash_of_the_secret_is_kept() {
    let (s, user) = store_with_user().await;
    let (_, secret) = s
        .token_create(user, "ci", &[Permission::HostView], None)
        .await
        .unwrap();

    let (hash,): (Vec<u8>,) = sqlx::query_as("SELECT secret_hash FROM api_tokens")
        .fetch_one(s.pool())
        .await
        .unwrap();
    assert_eq!(hash.len(), 32);
    assert!(!String::from_utf8_lossy(&hash).contains(&secret[7..]));
}

#[tokio::test]
async fn two_tokens_never_share_a_secret() {
    let (s, user) = store_with_user().await;
    let (_, a) = s
        .token_create(user, "a", &[Permission::HostView], None)
        .await
        .unwrap();
    let (_, b) = s
        .token_create(user, "b", &[Permission::HostView], None)
        .await
        .unwrap();
    assert_ne!(a, b);
}

#[tokio::test]
async fn an_unknown_or_mangled_secret_authenticates_nobody() {
    let (s, user) = store_with_user().await;
    let (_, secret) = s
        .token_create(user, "ci", &[Permission::HostView], None)
        .await
        .unwrap();

    assert!(
        s.token_authenticate("ghostdock_nope")
            .await
            .unwrap()
            .is_none()
    );
    let mangled = format!("{}x", &secret[..secret.len() - 1]);
    assert!(s.token_authenticate(&mangled).await.unwrap().is_none());
}

#[tokio::test]
async fn an_expired_token_authenticates_nobody() {
    let (s, user) = store_with_user().await;
    let past = chrono::Utc::now().timestamp() - 1;
    let (_, secret) = s
        .token_create(user, "old", &[Permission::HostView], Some(past))
        .await
        .unwrap();
    assert!(s.token_authenticate(&secret).await.unwrap().is_none());
}

#[tokio::test]
async fn a_revoked_token_authenticates_nobody() {
    let (s, user) = store_with_user().await;
    let (row, secret) = s
        .token_create(user, "ci", &[Permission::HostView], None)
        .await
        .unwrap();
    s.token_delete(user, row.id).await.unwrap();
    assert!(s.token_authenticate(&secret).await.unwrap().is_none());
}

#[tokio::test]
async fn a_token_can_only_be_revoked_by_its_owner() {
    let (s, user) = store_with_user().await;
    let other = s.user_create("other", "h").await.unwrap();
    let (row, _) = s
        .token_create(user, "ci", &[Permission::HostView], None)
        .await
        .unwrap();

    let err = s.token_delete(other.id, row.id).await.unwrap_err();
    assert!(matches!(err, store::Error::NotFound), "{err:?}");
    assert_eq!(s.tokens_for_user(user).await.unwrap().len(), 1);
}

#[tokio::test]
async fn removing_an_account_removes_its_tokens() {
    let (s, _) = store_with_user().await;
    let doomed = s.user_create("doomed", "h").await.unwrap();
    let (_, secret) = s
        .token_create(doomed.id, "ci", &[Permission::HostView], None)
        .await
        .unwrap();

    s.user_delete(doomed.id).await.unwrap();
    assert!(s.token_authenticate(&secret).await.unwrap().is_none());
}

#[tokio::test]
async fn token_names_are_unique_per_account() {
    let (s, user) = store_with_user().await;
    s.token_create(user, "ci", &[Permission::HostView], None)
        .await
        .unwrap();
    let err = s
        .token_create(user, "ci", &[Permission::HostView], None)
        .await
        .unwrap_err();
    assert!(matches!(err, store::Error::NameTaken), "{err:?}");
}

#[tokio::test]
async fn using_a_token_records_when() {
    let (s, user) = store_with_user().await;
    let (row, secret) = s
        .token_create(user, "ci", &[Permission::HostView], None)
        .await
        .unwrap();
    assert!(row.last_used_at.is_none());

    s.token_authenticate(&secret).await.unwrap();
    let listed = s.tokens_for_user(user).await.unwrap();
    assert!(listed[0].last_used_at.is_some());
}
