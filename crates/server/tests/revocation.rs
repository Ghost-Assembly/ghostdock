//! Live connections end when the right to them does.

use std::time::Duration;

use server::auth::{Principal, SessionKey, Via};
use server::revocation::{Revocation, Revocations};
use shared::auth::User;

fn user(id: i64) -> User {
    User {
        id,
        username: "a".to_owned(),
    }
}

/// A browser session of `user`, told apart from its others by `key`.
fn session_with(user_id: i64, key: i128) -> Principal {
    Principal {
        user: user(user_id),
        via: Via::Session {
            session: Some(SessionKey(key)),
        },
    }
}

fn session(user_id: i64) -> Principal {
    session_with(user_id, i128::from(user_id) * 1000)
}

fn token_expiring(user_id: i64, id: i64, expires_at: Option<i64>) -> Principal {
    Principal {
        user: user(user_id),
        via: Via::Token {
            id,
            name: "t".to_owned(),
            permissions: Vec::new(),
            expires_at,
        },
    }
}

fn token(user_id: i64, id: i64) -> Principal {
    token_expiring(user_id, id, None)
}

fn now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

/// Whether `revocation` ends a connection held by `principal`.
async fn ends(principal: &Principal, revocation: Revocation) -> bool {
    let revocations = Revocations::new();
    let waiting = revocations.until_revoked(principal);
    revocations.revoke(revocation);
    tokio::time::timeout(Duration::from_millis(200), waiting)
        .await
        .is_ok()
}

/// Whether a connection held by `principal` ends within `within` with
/// nothing revoked.
async fn ends_by_itself(principal: &Principal, within: Duration) -> bool {
    let revocations = Revocations::new();
    tokio::time::timeout(within, revocations.until_revoked(principal))
        .await
        .is_ok()
}

#[tokio::test]
async fn revoking_a_token_ends_that_tokens_connections_only() {
    assert!(ends(&token(1, 7), Revocation::Token(7)).await);
    assert!(!ends(&token(1, 8), Revocation::Token(7)).await);
    assert!(!ends(&session(1), Revocation::Token(7)).await);
}

#[tokio::test]
async fn removing_an_account_ends_everything_it_holds() {
    assert!(ends(&session(1), Revocation::Account(1)).await);
    assert!(ends(&token(1, 7), Revocation::Account(1)).await);
    assert!(!ends(&session(2), Revocation::Account(1)).await);
}

#[tokio::test]
async fn a_password_change_ends_sessions_but_not_tokens() {
    // Tokens were never derived from the password, so changing it is no
    // reason to break an integration.
    assert!(ends(&session(1), Revocation::Sessions(1)).await);
    assert!(!ends(&token(1, 7), Revocation::Sessions(1)).await);
    assert!(!ends(&session(2), Revocation::Sessions(1)).await);
}

#[tokio::test]
async fn signing_out_ends_that_sessions_connections_only() {
    // The same person on another device stays signed in, and so do its
    // connections.
    let signed_out = Revocation::Session(SessionKey(41));
    assert!(ends(&session_with(1, 41), signed_out).await);
    assert!(!ends(&session_with(1, 42), signed_out).await);
    assert!(!ends(&token(1, 41), signed_out).await);
}

#[tokio::test]
async fn a_tokens_connections_end_when_it_expires() {
    // A request made with it would be refused from then on; a connection
    // it opened must not carry on regardless.
    assert!(
        ends_by_itself(
            &token_expiring(1, 7, Some(now() - 5)),
            Duration::from_millis(200)
        )
        .await
    );
    assert!(
        ends_by_itself(
            &token_expiring(1, 7, Some(now() + 1)),
            Duration::from_secs(3)
        )
        .await
    );
    assert!(
        !ends_by_itself(
            &token_expiring(1, 7, Some(now() + 3600)),
            Duration::from_millis(200)
        )
        .await
    );
    assert!(!ends_by_itself(&token(1, 7), Duration::from_millis(200)).await);
    assert!(!ends_by_itself(&session(1), Duration::from_millis(200)).await);
}
