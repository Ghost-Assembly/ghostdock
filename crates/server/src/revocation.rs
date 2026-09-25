//! Ending live connections when the right to them is withdrawn.
//!
//! Ordinary requests re-check the caller every time, so revoking a token or
//! removing an account takes effect on the next one. A shell or an event
//! stream is a single request that can stay open for hours; without this,
//! it would outlive the credential that opened it.

use std::future::Future;
use std::time::Duration;

use tokio::sync::broadcast;

use crate::auth::{Principal, SessionKey, Via};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revocation {
    /// One token was revoked.
    Token(i64),
    /// An account was removed: its sessions and its tokens are gone.
    Account(i64),
    /// An account's password changed: its sessions are void, its tokens
    /// are not.
    Sessions(i64),
    /// One session signed out.
    Session(SessionKey),
}

impl Revocation {
    fn applies_to(self, principal: &Principal) -> bool {
        match (self, &principal.via) {
            (Self::Token(id), Via::Token { id: held, .. }) => id == *held,
            (Self::Account(user), _) => user == principal.user.id,
            (Self::Sessions(user), Via::Session { .. }) => user == principal.user.id,
            (
                Self::Session(key),
                Via::Session {
                    session: Some(held),
                },
            ) => key == *held,
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Revocations {
    tx: broadcast::Sender<Revocation>,
}

impl Default for Revocations {
    fn default() -> Self {
        Self::new()
    }
}

impl Revocations {
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(64);
        Self { tx }
    }

    pub fn revoke(&self, revocation: Revocation) {
        // No receivers just means nothing is open right now.
        let _ = self.tx.send(revocation);
    }

    /// Completes when a revocation covering `principal` is announced, or
    /// when the token it holds expires.
    ///
    /// Subscribes immediately, not when first polled, so nothing announced
    /// between this call and the connection starting is missed.
    pub fn until_revoked(&self, principal: &Principal) -> impl Future<Output = ()> + Send + use<> {
        let mut rx = self.tx.subscribe();
        let expires_at = match &principal.via {
            Via::Token { expires_at, .. } => *expires_at,
            Via::Session { .. } => None,
        };
        let principal = principal.clone();
        let announced = async move {
            loop {
                match rx.recv().await {
                    Ok(r) if r.applies_to(&principal) => return,
                    Ok(_) => {}
                    // Missed announcements might have included ours. Ending
                    // a connection that should have lived is recoverable (it
                    // reconnects); keeping one that should have died is not.
                    Err(broadcast::error::RecvError::Lagged(_)) => return,
                    Err(broadcast::error::RecvError::Closed) => {
                        std::future::pending::<()>().await;
                    }
                }
            }
        };
        async move {
            tokio::select! {
                () = announced => {}
                () = expiry(expires_at) => {}
            }
        }
    }
}

/// Completes at `expires_at` (Unix seconds), or never without one.
async fn expiry(expires_at: Option<i64>) {
    let Some(expires_at) = expires_at else {
        return std::future::pending().await;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let left = u64::try_from(expires_at.saturating_sub(now)).unwrap_or(0);
    tokio::time::sleep(Duration::from_secs(left)).await;
}
