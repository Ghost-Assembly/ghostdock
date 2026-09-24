//! Live connections end when the right to them does.

use std::time::Duration;

use server::auth::{Principal, Via};
use server::revocation::{Revocation, Revocations};
use shared::auth::User;

fn session(user: i64) -> Principal {
    Principal {
        user: User {
            id: user,
            username: "a".to_owned(),
        },
        via: Via::Session,
    }
}

fn token(user: i64, id: i64) -> Principal {
    Principal {
        user: User {
            id: user,
            username: "a".to_owned(),
        },
        via: Via::Token {
            id,
            name: "t".to_owned(),
            permissions: Vec::new(),
        },
    }
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
